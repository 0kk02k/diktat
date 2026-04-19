use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::OnceLock;
use tauri::{AppHandle, Emitter};
use tracing::{info, warn, debug, error};

const OLLAMA_BASE_URL: &str = "http://localhost:11434";
const DEFAULT_MODEL: &str = "gemma4:latest";
const DEFAULT_TIMEOUT_SECS: u64 = 300;
const DEFAULT_NUM_CTX: u32 = 32768;
/// Keep-Alive: Wie lange Ollama das Modell nach letzter Anfrage im Speicher behaelt
/// "5m" = 5 Minuten, "0" = sofort entladen, "-1" = fuer immer behalten
const DEFAULT_KEEP_ALIVE: &str = "5m";

/// Globaler HTTP-Client mit Connection-Pooling
/// Wird mit dem Analyse-Timeout erstellt (300s). Fuer kurze Checks
/// wird ein separater Client verwendet.
static HTTP_CLIENT: OnceLock<Client> = OnceLock::new();

/// Kurzzeit-Client fuer Status-Checks (5s Timeout)
static CHECK_CLIENT: OnceLock<Client> = OnceLock::new();

fn get_http_client() -> &'static Client {
    HTTP_CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .pool_max_idle_per_host(2)
            .pool_idle_timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("HTTP-Client konnte nicht erstellt werden")
    })
}

fn get_check_client() -> &'static Client {
    CHECK_CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .connect_timeout(std::time::Duration::from_secs(5))
            .build()
            .expect("Check-Client konnte nicht erstellt werden")
    })
}

/// Analyse-Tasks mit System-Prompts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AnalysisTask {
    /// Kurze Zusammenfassung
    Summary,
    /// Ausfuehrliche Zusammenfassung
    DetailedSummary,
    /// Themen und Keywords extrahieren
    Topics,
    /// Aktionspunkte / To-dos
    Actions,
    /// Stimmungsanalyse
    Sentiment,
    /// Beschluesse extrahieren
    Decisions,
    /// Protokoll erstellen
    Protocol,
    /// Alle Analysen kombiniert
    Full,
}

impl AnalysisTask {
    pub fn from_str(s: &str) -> Self {
        match s {
            "summary" => Self::Summary,
            "detailed_summary" => Self::DetailedSummary,
            "topics" => Self::Topics,
            "actions" => Self::Actions,
            "sentiment" => Self::Sentiment,
            "decisions" => Self::Decisions,
            "protocol" => Self::Protocol,
            "full" => Self::Full,
            _ => Self::Summary,
        }
    }

    pub fn system_prompt(&self) -> &'static str {
        match self {
            Self::Summary => "Du bist ein professioneller Assistent fuer Transkript-Analyse. \
                Erstelle eine kurze, praegnante Zusammenfassung des folgenden Transkripts auf Deutsch. \
                Maximal 3-5 Absaetze. Konzentriere dich auf die Kernaussagen.",

            Self::DetailedSummary => "Du bist ein professioneller Assistent fuer Transkript-Analyse. \
                Erstelle eine ausfuehrliche Zusammenfassung des folgenden Transkripts auf Deutsch. \
                Strukturiere sie mit Ueberschriften und Abschnitten. \
                Gehe auf alle wichtigen Punkte detailliert ein.",

            Self::Topics => "Du bist ein professioneller Assistent fuer Transkript-Analyse. \
                Extrahiere alle Hauptthemen und Keywords aus dem folgenden Transkript. \
                Gib jedes Thema als Stichpunkt mit einer kurzen Erklaerung aus. \
                Sortiere nach Wichtigkeit.",

            Self::Actions => "Du bist ein professioneller Assistent fuer Transkript-Analyse. \
                Extrahiere alle Aktionspunkte, To-dos, Aufgaben und Verantwortlichkeiten \
                aus dem folgenden Transkript. Gib sie als nummerierte Liste aus. \
                Wenn Personen genannt werden, ordne die Aufgaben ihnen zu.",

            Self::Sentiment => "Du bist ein professioneller Assistent fuer Stimmungsanalyse. \
                Analysiere die Stimmung und den Ton des folgenden Transkripts. \
                Beschreibe: 1) Die generelle emotionale Tendenz (positiv/neutral/negativ), \
                2) Markante Stimmungswechsel, 3) Die Dominanz einzelner Sprecher falls erkennbar.",

            Self::Decisions => "Du bist ein professioneller Assistent fuer Transkript-Analyse. \
                Extrahiere alle Beschluesse, Entscheidungen und Einigungen aus dem folgenden Transkript. \
                Gib jeden Beschluss als nummerierten Punkt aus mit: \
                - Was wurde beschlossen? \
                - Wer ist verantwortlich? (falls genannt) \
                - Gibt es eine Frist? (falls genannt)",

            Self::Protocol => "Du bist ein professioneller Protokollfuehrer. \
                Erstelle ein strukturiertes Protokoll aus dem folgenden Transkript. \
                Verwende dieses Format:\n\
                ## Protokoll\n\
                ### Teilnehmer\n\
                (falls erkennbar)\n\
                ### Agenda / Themen\n\
                (Hauptthemen auflisten)\n\
                ### Diskussion\n\
                (Zusammenfassung der Diskussionen pro Thema)\n\
                ### Beschluesse\n\
                (Entscheidungen und Ergebnisse)\n\
                ### Aktionspunkte\n\
                (Aufgaben mit Verantwortlichen und Fristen)",

            Self::Full => "Du bist ein professioneller Assistent fuer Transkript-Analyse. \
                Erstelle eine umfassende Analyse des folgenden Transkripts mit folgenden Abschnitten:\n\
                1. Zusammenfassung (kurz)\n\
                2. Hauptthemen\n\
                3. Aktionspunkte\n\
                4. Beschluesse\n\
                5. Stimmungsanalyse",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Summary => "Zusammenfassung",
            Self::DetailedSummary => "Ausfuehrliche Zusammenfassung",
            Self::Topics => "Themen",
            Self::Actions => "Aktionspunkte",
            Self::Sentiment => "Stimmungsanalyse",
            Self::Decisions => "Beschluesse",
            Self::Protocol => "Protokoll",
            Self::Full => "Vollanalyse",
        }
    }
}

/// Prueft ob Ollama erreichbar ist und gibt Status + Modelle zurueck
pub async fn check_status() -> Result<serde_json::Value, String> {
    let client = get_check_client();

    let response = client
        .get(format!("{}/api/tags", OLLAMA_BASE_URL))
        .send()
        .await
        .map_err(|e| format!("Ollama nicht erreichbar: {}", e))?;

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Antwort konnte nicht gelesen werden: {}", e))?;

    Ok(body)
}

/// Analysiert ein Transkript mit dem angegebenen Task (nicht-streaming)
pub async fn analyze(transcript: &str, task: &str) -> Result<String, String> {
    // Input-Validierung
    if transcript.trim().is_empty() {
        return Err("Transkript ist leer. Nichts zu analysieren.".to_string());
    }
    if transcript.len() > 500_000 {
        return Err(format!(
            "Transkript zu lang: {} Zeichen. Maximum: 500.000 Zeichen.",
            transcript.len()
        ));
    }

    let analysis_task = AnalysisTask::from_str(task);
    let system_prompt = analysis_task.system_prompt();

    info!(
        "Starte Analyse: {} ({} Zeichen, Task: {})",
        analysis_task.label(),
        transcript.len(),
        task
    );
    debug!(
        "Analyse User-Message (erste 200 Zeichen): {:?}",
        &transcript[..transcript.len().min(200)]
    );

    let client = get_http_client();

    let request_body = json!({
        "model": DEFAULT_MODEL,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": transcript}
        ],
        "stream": false,
        "keep_alive": DEFAULT_KEEP_ALIVE,
        "options": {
            "num_ctx": DEFAULT_NUM_CTX,
            "temperature": 0.3,
            "top_p": 0.9
        }
    });
    info!("Ollama Request: model={}, system_prompt_len={}, user_msg_len={}",
        DEFAULT_MODEL, system_prompt.len(), transcript.len());

    let response = client
        .post(format!("{}/api/chat", OLLAMA_BASE_URL))
        .json(&request_body)
        .send()
        .await
        .map_err(|e| {
            error!("Ollama Request-Fehler: {} (connect={}, timeout={})", e, e.is_connect(), e.is_timeout());
            if e.is_connect() {
                "Ollama ist nicht erreichbar. Bitte starte Ollama mit 'ollama serve'.".to_string()
            } else if e.is_timeout() {
                "Ollama-Anfrage hat das Zeitlimit ueberschritten. Versuche es mit einem kuerzeren Text.".to_string()
            } else {
                format!("Ollama-Fehler: {}", e)
            }
        })?;

    let status = response.status();
    info!("Ollama Response Status: {}", status);

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Antwort konnte nicht gelesen werden: {}", e))?;

    info!("Ollama Response Keys: {:?}", body.as_object().map(|o| o.keys().collect::<Vec<_>>()));

    let content = body["message"]["content"]
        .as_str()
        .unwrap_or("Keine Antwort von Ollama erhalten")
        .to_string();

    info!(
        "Analyse abgeschlossen: {} ({} Zeichen)",
        analysis_task.label(),
        content.len()
    );

    Ok(content)
}

/// Analysiert ein Transkript mit Streaming-Ausgabe
/// Sendet jeden Token als Event an das Frontend
pub async fn analyze_stream(
    app: &AppHandle,
    transcript: &str,
    task: &str,
) -> Result<String, String> {
    // Input-Validierung
    if transcript.trim().is_empty() {
        return Err("Transkript ist leer. Nichts zu analysieren.".to_string());
    }
    if transcript.len() > 500_000 {
        return Err(format!(
            "Transkript zu lang: {} Zeichen. Maximum: 500.000 Zeichen.",
            transcript.len()
        ));
    }

    let analysis_task = AnalysisTask::from_str(task);
    let system_prompt = analysis_task.system_prompt();

    info!(
        "Starte Streaming-Analyse: {} ({} Zeichen, Task: {})",
        analysis_task.label(),
        transcript.len(),
        task
    );
    debug!(
        "Stream User-Message (erste 200 Zeichen): {:?}",
        &transcript[..transcript.len().min(200)]
    );

    let client = get_http_client();

    let request_body = json!({
        "model": DEFAULT_MODEL,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": transcript}
        ],
        "stream": true,
        "keep_alive": DEFAULT_KEEP_ALIVE,
        "options": {
            "num_ctx": DEFAULT_NUM_CTX,
            "temperature": 0.3,
            "top_p": 0.9
        }
    });
    info!("Ollama Stream Request: model={}, system_prompt_len={}, user_msg_len={}",
        DEFAULT_MODEL, system_prompt.len(), transcript.len());

    let response = client
        .post(format!("{}/api/chat", OLLAMA_BASE_URL))
        .json(&request_body)
        .send()
        .await
        .map_err(|e| {
            error!("Ollama Stream Request-Fehler: {} (connect={}, timeout={})", e, e.is_connect(), e.is_timeout());
            if e.is_connect() {
                "Ollama ist nicht erreichbar. Bitte starte Ollama mit 'ollama serve'.".to_string()
            } else if e.is_timeout() {
                "Ollama-Anfrage hat das Zeitlimit ueberschritten.".to_string()
            } else {
                format!("Ollama-Fehler: {}", e)
            }
        })?;

    let status = response.status();
    info!("Ollama Stream Response Status: {}", status);

    // Streaming: Zeile fuer Zeile lesen
    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("Streaming-Antwort konnte nicht gelesen werden: {}", e))?;

    let body_text = String::from_utf8_lossy(&bytes);
    let mut full_response = String::new();

    for line in body_text.lines() {
        if line.trim().is_empty() {
            continue;
        }

        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(json) => {
                if let Some(content) = json["message"]["content"].as_str() {
                    full_response.push_str(content);

                    // Token-Event an Frontend senden
                    let _ = app.emit(
                        "analysis-token",
                        serde_json::json!({
                            "token": content,
                            "task": task,
                            "accumulated": full_response.len(),
                        }),
                    );
                }

                // Pruefen ob Stream beendet
                if json["done"].as_bool().unwrap_or(false) {
                    break;
                }
            }
            Err(e) => {
                warn!("Ungueltige JSON-Zeile im Stream: {} ({})", line, e);
            }
        }
    }

    info!(
        "Streaming-Analyse abgeschlossen: {} ({} Zeichen)",
        analysis_task.label(),
        full_response.len()
    );

    // Abschluss-Event senden
    let _ = app.emit(
        "analysis-complete",
        serde_json::json!({
            "task": task,
            "result": full_response,
        }),
    );

    Ok(full_response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analysis_task_from_str() {
        assert!(matches!(AnalysisTask::from_str("summary"), AnalysisTask::Summary));
        assert!(matches!(AnalysisTask::from_str("topics"), AnalysisTask::Topics));
        assert!(matches!(AnalysisTask::from_str("actions"), AnalysisTask::Actions));
        assert!(matches!(AnalysisTask::from_str("sentiment"), AnalysisTask::Sentiment));
        assert!(matches!(AnalysisTask::from_str("decisions"), AnalysisTask::Decisions));
        assert!(matches!(AnalysisTask::from_str("protocol"), AnalysisTask::Protocol));
        assert!(matches!(AnalysisTask::from_str("full"), AnalysisTask::Full));
        // Unknown -> Summary
        assert!(matches!(AnalysisTask::from_str("unknown"), AnalysisTask::Summary));
    }

    #[test]
    fn test_system_prompts_not_empty() {
        let tasks = vec!["summary", "topics", "actions", "sentiment", "decisions", "protocol", "full"];
        for task in tasks {
            let t = AnalysisTask::from_str(task);
            assert!(!t.system_prompt().is_empty(), "Prompt fuer {} ist leer", task);
        }
    }

    #[test]
    fn test_labels() {
        assert_eq!(AnalysisTask::Summary.label(), "Zusammenfassung");
        assert_eq!(AnalysisTask::Protocol.label(), "Protokoll");
        assert_eq!(AnalysisTask::Full.label(), "Vollanalyse");
    }
}
