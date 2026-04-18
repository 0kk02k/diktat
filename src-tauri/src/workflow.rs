use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tracing::{error, info};

use crate::audio;
use crate::ollama;
use crate::whisper;

/// Workflow-Phasen
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WorkflowPhase {
    Idle,
    LoadingAudio,
    Transcribing,
    Analyzing,
    Complete,
    Error,
}

/// Gesamtzustand des Workflows
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowState {
    pub phase: WorkflowPhase,
    pub audio_file: Option<String>,
    pub audio_info: Option<audio::AudioInfo>,
    pub total_chunks: usize,
    pub chunks_done: usize,
    pub transcript: Option<String>,
    pub analysis_task: Option<String>,
    pub analysis_result: Option<String>,
    pub error: Option<String>,
}

impl Default for WorkflowState {
    fn default() -> Self {
        Self {
            phase: WorkflowPhase::Idle,
            audio_file: None,
            audio_info: None,
            total_chunks: 0,
            chunks_done: 0,
            transcript: None,
            analysis_task: None,
            analysis_result: None,
            error: None,
        }
    }
}

/// Ergebnis des kompletten Workflows
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowResult {
    pub transcript: String,
    pub analysis: String,
    pub audio_info: audio::AudioInfo,
    pub chunks_total: usize,
    pub language: String,
    pub task: String,
}

/// Tauri-Command: Fuehrt den kompletten Workflow aus (Transkription + Analyse)
/// Sendet kontinuierlich Workflow-State-Updates an das Frontend
#[tauri::command]
pub async fn run_workflow(
    app: AppHandle,
    path: String,
    task: String,
    language: Option<String>,
) -> Result<WorkflowResult, String> {
    let lang = language.unwrap_or_else(|| "de".to_string());

    info!(
        "Workflow gestartet: {} (Sprache: {}, Task: {})",
        path, lang, task
    );

    // --- Pre-Checks ---

    // Pruefe ob Audio-Datei existiert
    let audio_path = std::path::Path::new(&path);
    if !audio_path.exists() {
        let err_msg = format!("Audiodatei nicht gefunden: {}", path);
        error!("{}", err_msg);
        emit_workflow_state(&app, WorkflowPhase::Error, 0, 0, None, None);
        return Err(err_msg);
    }

    // Pruefe ob Whisper-Modell existiert
    let mpath = whisper::default_model_path();
    if !mpath.exists() {
        let err_msg = format!(
            "Whisper-Modell nicht gefunden: {}. Bitte lade es herunter.",
            mpath.display()
        );
        error!("{}", err_msg);
        emit_workflow_state(&app, WorkflowPhase::Error, 0, 0, None, None);
        return Err(err_msg);
    }

    // Pruefe ob Ollama erreichbar ist
    match ollama::check_status().await {
        Ok(_) => info!("Ollama erreichbar, starte Workflow"),
        Err(e) => {
            let err_msg = format!(
                "Ollama ist nicht erreichbar: {}. Bitte starte Ollama mit 'ollama serve'.",
                e
            );
            error!("{}", err_msg);
            emit_workflow_state(&app, WorkflowPhase::Error, 0, 0, None, None);
            return Err(err_msg);
        }
    }

    // --- Phase: Audio laden ---
    emit_workflow_state(&app, WorkflowPhase::LoadingAudio, 0, 0, None, None);
    let audio_path_owned = path.clone();
    let (info, samples) = tokio::task::spawn_blocking(move || {
        audio::load_audio(std::path::Path::new(&audio_path_owned))
    })
    .await
    .map_err(|e| format!("Audio-Laden fehlgeschlagen: {}", e))??;

    info!(
        "Audio geladen: {} ({:.1}s, {} Hz)",
        info.filename, info.duration_secs, info.sample_rate
    );

    let chunks = audio::chunk_audio(&samples, info.sample_rate);
    let total_chunks = chunks.len();

    emit_workflow_state(
        &app,
        WorkflowPhase::Transcribing,
        0,
        total_chunks,
        None,
        None,
    );

    // --- Phase 1: Transkription ---
    info!("Lade Whisper-Modell: {:?}", mpath);

    // Whisper-Modell laden
    let state_mutex = app.state::<std::sync::Mutex<whisper::WhisperState>>();
    {
        let mut whisper_state = state_mutex.lock().map_err(|e| e.to_string())?;
        whisper_state.load_model(&mpath)?;
    }

    let mut chunk_transcripts = Vec::new();

    for (i, chunk) in chunks.into_iter().enumerate() {
        let whisper_state = state_mutex.lock().map_err(|e| e.to_string())?;

        match whisper_state.transcribe_chunk(&chunk.samples, Some(&lang)) {
            Ok(text) => {
                chunk_transcripts.push(whisper::ChunkTranscript {
                    index: chunk.index,
                    start_secs: chunk.start_secs,
                    end_secs: chunk.end_secs,
                    text,
                    language: lang.clone(),
                });
            }
            Err(e) => {
                error!("Fehler bei Chunk {}: {}", i, e);
                chunk_transcripts.push(whisper::ChunkTranscript {
                    index: chunk.index,
                    start_secs: chunk.start_secs,
                    end_secs: chunk.end_secs,
                    text: format!("[Fehler: {}]", e),
                    language: lang.clone(),
                });
            }
        }

        drop(whisper_state);

        // Fortschritt senden
        let progress = ((i + 1) as f64 / total_chunks as f64) * 100.0;
        let current_text = chunk_transcripts
            .last()
            .map(|c| c.text.clone())
            .unwrap_or_default();

        // Transkriptions-Fortschritt (spezifisch)
        let _ = app.emit(
            "transcription-progress",
            serde_json::json!({
                "chunk_index": i,
                "total_chunks": total_chunks,
                "progress_percent": progress,
                "current_text": current_text,
            }),
        );

        // Workflow-State-Update
        emit_workflow_state(
            &app,
            WorkflowPhase::Transcribing,
            i + 1,
            total_chunks,
            None,
            None,
        );
    }

    // Gesamttranskript zusammenfuegen
    let full_text = whisper::merge_transcripts(&chunk_transcripts);

    info!(
        "Transkription abgeschlossen: {} Zeichen aus {:.1}s Audio",
        full_text.len(),
        info.duration_secs
    );

    // Transkription-Complete Event
    let _ = app.emit(
        "transcription-complete",
        serde_json::json!({
            "full_text": full_text,
            "total_duration_secs": info.duration_secs,
            "chunks": chunk_transcripts,
            "language": lang,
        }),
    );

    // --- Phase 2: Analyse ---
    emit_workflow_state(
        &app,
        WorkflowPhase::Analyzing,
        total_chunks,
        total_chunks,
        Some(&full_text),
        None,
    );

    let analysis = ollama::analyze_stream(&app, &full_text, &task).await?;

    info!(
        "Analyse abgeschlossen: {} ({} Zeichen)",
        task,
        analysis.len()
    );

    // --- Phase: Complete ---
    emit_workflow_state(
        &app,
        WorkflowPhase::Complete,
        total_chunks,
        total_chunks,
        Some(&full_text),
        Some(&analysis),
    );

    // Workflow-Complete Event
    let _ = app.emit(
        "workflow-complete",
        serde_json::json!({
            "transcript": full_text,
            "analysis": analysis,
            "task": task,
        }),
    );

    Ok(WorkflowResult {
        transcript: full_text,
        analysis,
        audio_info: info,
        chunks_total: total_chunks,
        language: lang,
        task,
    })
}

/// Sendet den aktuellen Workflow-State an das Frontend
fn emit_workflow_state(
    app: &AppHandle,
    phase: WorkflowPhase,
    chunks_done: usize,
    total_chunks: usize,
    transcript: Option<&str>,
    analysis: Option<&str>,
) {
    let state = WorkflowState {
        phase,
        audio_file: None,
        audio_info: None,
        total_chunks,
        chunks_done,
        transcript: transcript.map(|s| s.to_string()),
        analysis_task: None,
        analysis_result: analysis.map(|s| s.to_string()),
        error: None,
    };
    let _ = app.emit("workflow-state", &state);
}
