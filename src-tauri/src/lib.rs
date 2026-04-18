use tauri::Emitter;
use tracing::{info, warn};

mod audio;
mod export;
mod ollama;
mod whisper;
mod workflow;

use std::sync::Mutex;

#[tauri::command]
async fn check_ollama_status() -> Result<serde_json::Value, String> {
    ollama::check_status().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_ollama_models() -> Result<serde_json::Value, String> {
    ollama::check_status().await
}

#[tauri::command]
async fn analyze_transcript(transcript: String, task: String) -> Result<String, String> {
    ollama::analyze(&transcript, &task).await
}

/// Streaming-Analyse: sendet Token-Events an das Frontend
#[tauri::command]
async fn analyze_transcript_stream(
    app: tauri::AppHandle,
    transcript: String,
    task: String,
) -> Result<String, String> {
    ollama::analyze_stream(&app, &transcript, &task).await
}

/// Laedt eine Audiodatei und gibt Metadaten + Chunk-Anzahl zurueck
#[tauri::command]
async fn load_audio_file(path: String) -> Result<audio::AudioInfo, String> {
    let audio_path = std::path::Path::new(&path);
    let (info, _samples) = audio::load_audio(audio_path)?;
    Ok(info)
}

/// Laedt eine Audiodatei, teilt sie in Chunks und gibt Chunk-Metadaten zurueck
#[tauri::command]
async fn prepare_chunks(path: String) -> Result<serde_json::Value, String> {
    let audio_path = std::path::Path::new(&path);
    let (info, samples) = audio::load_audio(audio_path)?;
    let chunks = audio::chunk_audio(&samples, info.sample_rate);

    // Nur Metadaten zurueckgeben (nicht die Samples selbst)
    let chunk_meta: Vec<serde_json::Value> = chunks
        .iter()
        .map(|c| {
            serde_json::json!({
                "index": c.index,
                "start_secs": c.start_secs,
                "end_secs": c.end_secs,
                "sample_count": c.samples.len(),
            })
        })
        .collect();

    Ok(serde_json::json!({
        "audio_info": info,
        "chunks": chunk_meta,
    }))
}

/// Prueft ob das Whisper-Modell verfuegbar ist
#[tauri::command]
async fn check_whisper_model() -> Result<serde_json::Value, String> {
    let model_path = whisper::default_model_path();
    let exists = model_path.exists();
    let path_str = model_path.to_string_lossy().to_string();

    let file_size = if exists {
        std::fs::metadata(&model_path)
            .map(|m| m.len())
            .unwrap_or(0)
    } else {
        0
    };

    Ok(serde_json::json!({
        "exists": exists,
        "path": path_str,
        "file_size_bytes": file_size,
        "file_size_mb": if file_size > 0 { file_size as f64 / (1024.0 * 1024.0) } else { 0.0 },
        "valid": exists && file_size > 100_000, // Mindestens 100KB fuer ein gueltiges Modell
    }))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Mutex::new(whisper::WhisperState::new()))
        .invoke_handler(tauri::generate_handler![
            check_ollama_status,
            get_ollama_models,
            check_whisper_model,
            analyze_transcript,
            analyze_transcript_stream,
            load_audio_file,
            prepare_chunks,
            whisper::transcribe_audio,
            workflow::run_workflow,
            export::export_result,
            export::export_srt_file,
        ])
        .setup(|app| {
            info!("Diktat App gestartet");

            // Ollama-Verfuegbarkeit pruefen
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                match ollama::check_status().await {
                    Ok(body) => {
                        let model_count = body["models"]
                            .as_array()
                            .map(|m| m.len())
                            .unwrap_or(0);
                        info!("Ollama erreichbar: {} Modell(e) verfuegbar", model_count);
                    }
                    Err(e) => {
                        warn!("Ollama nicht erreichbar beim Start: {}", e);
                        let _ = handle.emit(
                            "startup-warning",
                            serde_json::json!({
                                "component": "ollama",
                                "message": "Ollama ist nicht erreichbar. Analyse-Funktionen sind deaktiviert. Starte Ollama mit 'ollama serve'."
                            }),
                        );
                    }
                }
            });

            // Whisper-Modell pruefen
            let model_path = whisper::default_model_path();
            if !model_path.exists() {
                warn!("Whisper-Modell nicht gefunden: {:?}", model_path);
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let _ = handle.emit(
                        "startup-warning",
                        serde_json::json!({
                            "component": "whisper",
                            "message": format!("Whisper-Modell nicht gefunden unter: {}. Bitte Modell herunterladen.", model_path.display())
                        }),
                    );
                });
            } else {
                info!("Whisper-Modell gefunden: {:?}", model_path);
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
