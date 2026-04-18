use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tracing::{error, info, warn};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::audio;

/// Ergebnis der Transkription eines einzelnen Chunks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkTranscript {
    pub index: usize,
    pub start_secs: f64,
    pub end_secs: f64,
    pub text: String,
    pub language: String,
}

/// Ergebnis der kompletten Transkription
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionResult {
    pub full_text: String,
    pub chunks: Vec<ChunkTranscript>,
    pub total_duration_secs: f64,
    pub language: String,
    pub model: String,
}

/// Zustand des Whisper-Modells (wird in Tauri State verwaltet)
pub struct WhisperState {
    ctx: Option<WhisperContext>,
    model_path: Option<PathBuf>,
}

impl WhisperState {
    pub fn new() -> Self {
        Self {
            ctx: None,
            model_path: None,
        }
    }

    /// Laedt das Whisper-Modell. Wird beim ersten Transkriptions-Aufruf gemacht (Lazy Loading).
    pub fn load_model(&mut self, model_path: &std::path::Path) -> Result<(), String> {
        if self.ctx.is_some() && self.model_path.as_deref() == Some(model_path) {
            info!("Whisper-Modell bereits geladen: {:?}", model_path);
            return Ok(());
        }

        info!("Lade Whisper-Modell: {:?}", model_path);
        if !model_path.exists() {
            return Err(format!(
                "Modell-Datei nicht gefunden: {}",
                model_path.display()
            ));
        }

        let ctx_params = WhisperContextParameters::default();
        let ctx = WhisperContext::new_with_params(
            model_path.to_string_lossy().as_ref(),
            ctx_params,
        )
        .map_err(|e| format!("Whisper-Modell konnte nicht geladen werden: {}", e))?;

        self.ctx = Some(ctx);
        self.model_path = Some(model_path.to_path_buf());
        info!("Whisper-Modell erfolgreich geladen");
        Ok(())
    }

    /// Transkribiert einen einzelnen Chunk
    pub fn transcribe_chunk(
        &self,
        samples: &[f32],
        language: Option<&str>,
    ) -> Result<String, String> {
        let ctx = self
            .ctx
            .as_ref()
            .ok_or("Whisper-Modell ist nicht geladen")?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

        // Sprache einstellen (Deutsch als Default)
        if let Some(lang) = language {
            params.set_language(Some(lang));
        } else {
            params.set_language(Some("de"));
        }

        // Transkription (nicht Uebersetzung)
        params.set_translate(false);

        // Kein Timestamp-Printing
        params.set_print_timestamps(false);

        // Single-Segment-Modus fuer Chunks
        params.set_single_segment(false);

        // Thread-Anzahl an CPU-Kerne anpassen
        let num_threads = std::thread::available_parallelism()
            .map(|n| n.get().min(8)) // Max 8 Threads
            .unwrap_or(4);
        params.set_n_threads(num_threads as i32);

        let mut state = ctx
            .create_state()
            .map_err(|e| format!("Whisper-State konnte nicht erstellt werden: {}", e))?;

        state
            .full(params, samples)
            .map_err(|e| format!("Whisper-Inferenz fehlgeschlagen: {}", e))?;

        let num_segments = state.full_n_segments();
        if num_segments < 0 {
            return Err(format!("Ungueltige Segment-Anzahl: {}", num_segments));
        }

        let mut text = String::new();
        for i in 0..num_segments {
            if let Some(segment) = state.get_segment(i) {
                match segment.to_str() {
                    Ok(s) => {
                        text.push_str(s);
                        text.push(' ');
                    }
                    Err(e) => {
                        warn!("Segment {} konnte nicht dekodiert werden: {}", i, e);
                    }
                }
            }
        }

        Ok(text.trim().to_string())
    }
}

/// Standard-Modellpfad
pub fn default_model_path() -> PathBuf {
    // Relativ zum Arbeitsverzeichnis, oder absolute Pfade pruefen
    let candidates = vec![
        PathBuf::from("models/ggml-large-v3-turbo.bin"),
        PathBuf::from("/home/okko/diktat/models/ggml-large-v3-turbo.bin"),
        PathBuf::from("../models/ggml-large-v3-turbo.bin"),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return candidate.clone();
        }
    }

    // Fallback: ersten Kandidaten zurueckgeben (wird beim Laden einen Fehler geben)
    candidates.into_iter().next().unwrap()
}

/// Tauri-Command: Transkribiert eine Audiodatei komplett
#[tauri::command]
pub async fn transcribe_audio(
    app: AppHandle,
    path: String,
    language: Option<String>,
    model_path: Option<String>,
) -> Result<TranscriptionResult, String> {
    let lang = language.as_deref().unwrap_or("de");
    let mpath = model_path
        .map(PathBuf::from)
        .unwrap_or_else(default_model_path);

    info!("Starte Transkription: {} (Sprache: {})", path, lang);

    // Audio laden (in blocking Task da es CPU-intensiv ist)
    let audio_path_owned = path.clone();
    let (info, samples) = tokio::task::spawn_blocking(move || {
        audio::load_audio(std::path::Path::new(&audio_path_owned))
    })
    .await
    .map_err(|e| format!("Audio-Laden fehlgeschlagen: {}", e))??;

    // Chunks erstellen
    let chunks = audio::chunk_audio(&samples, info.sample_rate);
    let total_chunks = chunks.len();
    info!("{} Chunks erstellt", total_chunks);

    // Whisper-State holen und Modell laden
    let state_mutex = app.state::<Mutex<WhisperState>>();
    {
        let mut whisper_state = state_mutex.lock().map_err(|e| e.to_string())?;
        whisper_state.load_model(&mpath)?;
    }

    // Chunks transkribieren
    let mut chunk_transcripts = Vec::new();

    for (i, chunk) in chunks.into_iter().enumerate() {
        info!(
            "Transkribiere Chunk {}/{}: {:.1}s - {:.1}s",
            i + 1,
            total_chunks,
            chunk.start_secs,
            chunk.end_secs
        );

        let whisper_state = state_mutex.lock().map_err(|e| e.to_string())?;

        match whisper_state.transcribe_chunk(&chunk.samples, Some(lang)) {
            Ok(text) => {
                let preview = if text.len() > 80 { &text[..80] } else { &text };
                info!("Chunk {}: \"{}\"", i, preview);
                chunk_transcripts.push(ChunkTranscript {
                    index: chunk.index,
                    start_secs: chunk.start_secs,
                    end_secs: chunk.end_secs,
                    text,
                    language: lang.to_string(),
                });
            }
            Err(e) => {
                error!("Fehler bei Chunk {}: {}", i, e);
                chunk_transcripts.push(ChunkTranscript {
                    index: chunk.index,
                    start_secs: chunk.start_secs,
                    end_secs: chunk.end_secs,
                    text: format!("[Fehler: {}]", e),
                    language: lang.to_string(),
                });
            }
        }

        drop(whisper_state);

        // Fortschritt an Frontend senden
        let progress = ((i + 1) as f64 / total_chunks as f64) * 100.0;
        let _ = app.emit(
            "transcription-progress",
            serde_json::json!({
                "chunk_index": i,
                "total_chunks": total_chunks,
                "progress_percent": progress,
                "current_text": chunk_transcripts.last().map(|c| c.text.clone()).unwrap_or_default(),
            }),
        );
    }

    // Gesamttranskript zusammenfuegen
    let full_text = merge_transcripts(&chunk_transcripts);

    let result = TranscriptionResult {
        full_text: full_text.clone(),
        chunks: chunk_transcripts,
        total_duration_secs: info.duration_secs,
        language: lang.to_string(),
        model: mpath
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default(),
    };

    info!(
        "Transkription abgeschlossen: {} Zeichen aus {:.1}s Audio",
        result.full_text.len(),
        result.total_duration_secs
    );

    // Abschluss-Event senden
    let _ = app.emit("transcription-complete", &result);

    Ok(result)
}

/// Fuegt Chunk-Transkripte zu einem Gesamttext zusammen
/// Entfernt doppelte Woerter an den Ueberlappungsgrenzen
pub fn merge_transcripts(chunks: &[ChunkTranscript]) -> String {
    if chunks.is_empty() {
        return String::new();
    }

    let mut merged = String::new();

    for (i, chunk) in chunks.iter().enumerate() {
        if i == 0 {
            merged = chunk.text.clone();
        } else {
            // Einfache Ueberlappungs-Bereinigung:
            // Versuche den letzten Satz/die letzten Woerter des bisherigen Textes
            // im neuen Chunk zu finden und schneide sie ab
            let appended = overlap_merge(&merged, &chunk.text);
            merged = appended;
        }
    }

    merged
}

/// Versucht Ueberlappungen zwischen zwei Texten zu bereinigen
fn overlap_merge(previous: &str, current: &str) -> String {
    let prev_words: Vec<&str> = previous.split_whitespace().collect();
    let curr_words: Vec<&str> = current.split_whitespace().collect();

    if prev_words.is_empty() {
        return current.to_string();
    }
    if curr_words.is_empty() {
        return previous.to_string();
    }

    // Suche nach der laengsten Ueberlappung der letzten Woerter des vorherigen Textes
    // mit den ersten Woertern des aktuellen Textes
    let max_overlap = std::cmp::min(prev_words.len(), curr_words.len()).min(20);

    let mut best_overlap = 0;
    for len in (2..=max_overlap).rev() {
        let prev_tail = &prev_words[prev_words.len() - len..];
        let curr_head = &curr_words[..len];

        // Case-insensitive Vergleich
        let matches = prev_tail
            .iter()
            .zip(curr_head.iter())
            .all(|(a, b)| a.to_lowercase() == b.to_lowercase());

        if matches {
            best_overlap = len;
            break;
        }
    }

    if best_overlap > 0 {
        // Ueberlappung gefunden: Woerter nach der Ueberlappung anhaengen
        let mut result = previous.to_string();
        for word in &curr_words[best_overlap..] {
            result.push(' ');
            result.push_str(word);
        }
        result
    } else {
        // Keine Ueberlappung: einfach anhaengen
        format!("{} {}", previous, current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_overlap_merge_no_overlap() {
        let result = overlap_merge("Hallo Welt", "Das ist ein Test");
        assert_eq!(result, "Hallo Welt Das ist ein Test");
    }

    #[test]
    fn test_overlap_merge_with_overlap() {
        let result = overlap_merge(
            "Hallo Welt das ist ein Test",
            "das ist ein Test und noch mehr",
        );
        assert_eq!(result, "Hallo Welt das ist ein Test und noch mehr");
    }

    #[test]
    fn test_overlap_merge_case_insensitive() {
        let result = overlap_merge("Hallo Welt Das Ist", "das ist ein Test");
        assert_eq!(result, "Hallo Welt Das Ist ein Test");
    }

    #[test]
    fn test_merge_transcripts_empty() {
        let result = merge_transcripts(&[]);
        assert_eq!(result, "");
    }

    #[test]
    fn test_merge_transcripts_single() {
        let chunks = vec![ChunkTranscript {
            index: 0,
            start_secs: 0.0,
            end_secs: 30.0,
            text: "Hallo Welt".to_string(),
            language: "de".to_string(),
        }];
        let result = merge_transcripts(&chunks);
        assert_eq!(result, "Hallo Welt");
    }

    #[test]
    fn test_default_model_path() {
        let path = default_model_path();
        // Sollte einen Pfad zurueckgeben (existiert moeglicherweise nicht im Test)
        assert!(path.to_string_lossy().contains("ggml-large-v3-turbo"));
    }
}
