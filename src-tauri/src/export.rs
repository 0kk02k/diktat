use serde::Serialize;
use std::path::{Path, PathBuf};
use tracing::info;

/// Ergebnis eines Exports
#[derive(Debug, Clone, Serialize)]
pub struct ExportResult {
    pub path: String,
    pub format: String,
    pub bytes_written: usize,
}

fn ensure_parent_dir(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Konnte Zielordner nicht erstellen: {}", e))?;
    }
    Ok(())
}

fn sanitize_filename_component(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();

    let sanitized = sanitized.trim_matches('_');
    if sanitized.is_empty() {
        "analyse".to_string()
    } else {
        sanitized.to_string()
    }
}

fn build_related_output_path(
    audio_path: &Path,
    suffix: &str,
    extension: &str,
) -> Result<PathBuf, String> {
    let parent = audio_path
        .parent()
        .ok_or_else(|| "Audiopfad hat keinen gueltigen Zielordner".to_string())?;
    let stem = audio_path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "Audiodatei hat keinen gueltigen Dateinamen".to_string())?;

    Ok(parent.join(format!("{stem}_{suffix}.{extension}")))
}

/// Exportiert ein Transkript als einfache Textdatei
pub fn export_txt(
    transcript: &str,
    analysis: Option<&str>,
    path: &std::path::Path,
) -> Result<ExportResult, String> {
    let mut content = String::new();
    content.push_str("=== Transkript ===\n\n");
    content.push_str(transcript);
    if let Some(a) = analysis {
        content.push_str("\n\n=== Analyse ===\n\n");
        content.push_str(a);
    }

    ensure_parent_dir(path)?;
    std::fs::write(path, &content).map_err(|e| format!("TXT-Export fehlgeschlagen: {}", e))?;

    info!("TXT exportiert: {:?} ({} Bytes)", path, content.len());
    Ok(ExportResult {
        path: path.to_string_lossy().to_string(),
        format: "txt".to_string(),
        bytes_written: content.len(),
    })
}

/// Exportiert als Markdown
pub fn export_markdown(
    transcript: &str,
    analysis: Option<&str>,
    audio_name: &str,
    path: &std::path::Path,
) -> Result<ExportResult, String> {
    let mut content = String::new();
    content.push_str(&format!("# Transkript: {}\n\n", audio_name));
    content.push_str("## Transkript\n\n");
    content.push_str(transcript);
    if let Some(a) = analysis {
        content.push_str("\n\n## Analyse\n\n");
        content.push_str(a);
    }
    content.push_str(&format!(
        "\n\n---\n*Erstellt mit Diktat am {}*\n",
        chrono::Local::now().format("%d.%m.%Y %H:%M")
    ));

    ensure_parent_dir(path)?;
    std::fs::write(path, &content).map_err(|e| format!("Markdown-Export fehlgeschlagen: {}", e))?;

    info!("Markdown exportiert: {:?} ({} Bytes)", path, content.len());
    Ok(ExportResult {
        path: path.to_string_lossy().to_string(),
        format: "md".to_string(),
        bytes_written: content.len(),
    })
}

/// Exportiert als JSON
pub fn export_json(
    transcript: &str,
    analysis: Option<&str>,
    audio_name: &str,
    chunks: Option<&serde_json::Value>,
    path: &std::path::Path,
) -> Result<ExportResult, String> {
    let data = serde_json::json!({
        "audio_file": audio_name,
        "created_at": chrono::Local::now().to_rfc3339(),
        "transcript": transcript,
        "analysis": analysis,
        "chunks": chunks,
    });

    let content = serde_json::to_string_pretty(&data)
        .map_err(|e| format!("JSON-Serialisierung fehlgeschlagen: {}", e))?;

    ensure_parent_dir(path)?;
    std::fs::write(path, &content).map_err(|e| format!("JSON-Export fehlgeschlagen: {}", e))?;

    info!("JSON exportiert: {:?} ({} Bytes)", path, content.len());
    Ok(ExportResult {
        path: path.to_string_lossy().to_string(),
        format: "json".to_string(),
        bytes_written: content.len(),
    })
}

/// Exportiert als SRT (Untertitel)
/// Benoetigt Chunk-Informationen mit Zeitstempeln
pub fn export_srt(
    chunks: &[(f64, f64, &str)],
    path: &std::path::Path,
) -> Result<ExportResult, String> {
    let mut content = String::new();

    for (i, (start, end, text)) in chunks.iter().enumerate() {
        content.push_str(&format!("{}\n", i + 1));
        content.push_str(&format!(
            "{} --> {}\n",
            format_srt_time(*start),
            format_srt_time(*end)
        ));
        content.push_str(text.trim());
        content.push_str("\n\n");
    }

    ensure_parent_dir(path)?;
    std::fs::write(path, &content).map_err(|e| format!("SRT-Export fehlgeschlagen: {}", e))?;

    info!("SRT exportiert: {:?} ({} Eintraege)", path, chunks.len());
    Ok(ExportResult {
        path: path.to_string_lossy().to_string(),
        format: "srt".to_string(),
        bytes_written: content.len(),
    })
}

/// Formatiert Sekunden als SRT-Zeitstempel (HH:MM:SS,mmm)
fn format_srt_time(secs: f64) -> String {
    let total_ms = (secs * 1000.0) as u64;
    let ms = total_ms % 1000;
    let total_secs = total_ms / 1000;
    let s = total_secs % 60;
    let total_mins = total_secs / 60;
    let m = total_mins % 60;
    let h = total_mins / 60;
    format!("{:02}:{:02}:{:02},{:03}", h, m, s, ms)
}

/// Tauri-Command: Exportiert das Transkript und die Analyse
#[tauri::command]
pub async fn export_result(
    transcript: String,
    analysis: Option<String>,
    audio_name: String,
    format: String,
    path: String,
) -> Result<ExportResult, String> {
    let export_path = std::path::Path::new(&path);

    match format.as_str() {
        "txt" => export_txt(&transcript, analysis.as_deref(), export_path),
        "md" => export_markdown(&transcript, analysis.as_deref(), &audio_name, export_path),
        "json" => export_json(
            &transcript,
            analysis.as_deref(),
            &audio_name,
            None,
            export_path,
        ),
        _ => Err(format!("Unbekanntes Export-Format: {}", format)),
    }
}

/// Tauri-Command: Exportiert als SRT mit Zeitstempel-Daten
#[tauri::command]
pub async fn export_srt_file(chunks_json: String, path: String) -> Result<ExportResult, String> {
    let chunks: Vec<serde_json::Value> =
        serde_json::from_str(&chunks_json).map_err(|e| format!("Ungueltige Chunk-Daten: {}", e))?;

    let srt_chunks: Vec<(f64, f64, &str)> = chunks
        .iter()
        .filter_map(|c| {
            let start = c.get("start_secs")?.as_f64()?;
            let end = c.get("end_secs")?.as_f64()?;
            let text = c.get("text")?.as_str()?;
            Some((start, end, text))
        })
        .collect();

    let export_path = std::path::Path::new(&path);
    export_srt(&srt_chunks, export_path)
}

#[tauri::command]
pub async fn auto_export_transcript(
    audio_path: String,
    transcript: String,
    chunks_json: Option<String>,
) -> Result<Vec<ExportResult>, String> {
    let audio_path = Path::new(&audio_path);
    let transcript_path = build_related_output_path(audio_path, "transkript", "txt")?;

    let mut results = vec![export_txt(&transcript, None, &transcript_path)?];

    if let Some(chunks_json) = chunks_json {
        let chunks: Vec<serde_json::Value> = serde_json::from_str(&chunks_json)
            .map_err(|e| format!("Ungueltige Chunk-Daten: {}", e))?;

        let srt_chunks: Vec<(f64, f64, &str)> = chunks
            .iter()
            .filter_map(|c| {
                let start = c.get("start_secs")?.as_f64()?;
                let end = c.get("end_secs")?.as_f64()?;
                let text = c.get("text")?.as_str()?;
                Some((start, end, text))
            })
            .collect();

        if !srt_chunks.is_empty() {
            let srt_path = build_related_output_path(audio_path, "untertitel", "srt")?;
            results.push(export_srt(&srt_chunks, &srt_path)?);
        }
    }

    Ok(results)
}

#[tauri::command]
pub async fn auto_export_analysis(
    audio_path: String,
    audio_name: String,
    transcript: String,
    analysis: String,
    task: String,
) -> Result<ExportResult, String> {
    let audio_path = Path::new(&audio_path);
    let task_slug = sanitize_filename_component(&task);
    let analysis_path =
        build_related_output_path(audio_path, &format!("analyse_{task_slug}"), "md")?;

    export_markdown(&transcript, Some(&analysis), &audio_name, &analysis_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_srt_time() {
        assert_eq!(format_srt_time(0.0), "00:00:00,000");
        assert_eq!(format_srt_time(61.5), "00:01:01,500");
        assert_eq!(format_srt_time(3661.123), "01:01:01,123");
        assert_eq!(format_srt_time(30.0), "00:00:30,000");
    }

    #[test]
    fn test_export_txt() {
        let dir = std::env::temp_dir().join("diktat_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.txt");
        let result = export_txt("Hallo Welt", Some("Analyse"), &path).unwrap();
        assert_eq!(result.format, "txt");
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("Hallo Welt"));
        assert!(content.contains("Analyse"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_export_markdown() {
        let dir = std::env::temp_dir().join("diktat_test_md");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.md");
        let result = export_markdown("Test Text", None, "audio.mp3", &path).unwrap();
        assert_eq!(result.format, "md");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# Transkript: audio.mp3"));
        assert!(content.contains("Test Text"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_export_srt() {
        let dir = std::env::temp_dir().join("diktat_test_srt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.srt");
        let chunks = vec![
            (0.0, 30.0, "Erster Chunk Text"),
            (28.5, 58.5, "Zweiter Chunk Text"),
        ];
        let result = export_srt(&chunks, &path).unwrap();
        assert_eq!(result.format, "srt");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("00:00:00,000 --> 00:00:30,000"));
        assert!(content.contains("Erster Chunk Text"));
        assert!(content.contains("00:00:28,500 --> 00:00:58,500"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_build_related_output_path() {
        let audio_path = Path::new("recordings/2026-04-20/aufnahme_20260420_140000.wav");
        let path = build_related_output_path(audio_path, "transkript", "txt").unwrap();
        assert_eq!(
            path,
            PathBuf::from("recordings/2026-04-20/aufnahme_20260420_140000_transkript.txt")
        );
    }

    #[test]
    fn test_sanitize_filename_component() {
        assert_eq!(
            sanitize_filename_component("Detailed Summary"),
            "detailed_summary"
        );
        assert_eq!(sanitize_filename_component(""), "analyse");
    }
}
