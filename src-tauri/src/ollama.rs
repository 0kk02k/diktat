use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::OnceLock;
use tauri::{AppHandle, Emitter};
use tracing::{debug, error, info, warn};

const OLLAMA_BASE_URL: &str = "http://localhost:11434";
const DEFAULT_MODEL: &str = "gemma4:latest";
const DEFAULT_TIMEOUT_SECS: u64 = 300;
const DEFAULT_NUM_CTX: u32 = 32768;
/// Keep-Alive: Wie lange Ollama das Modell nach letzter Anfrage im Speicher behaelt
/// "5m" = 5 Minuten, "0" = sofort entladen, "-1" = fuer immer behalten
const DEFAULT_KEEP_ALIVE: &str = "5m";

/// Global ausgewaehltes Ollama-Modell (kann zur Laufzeit geaendert werden)
static SELECTED_MODEL: OnceLock<std::sync::Mutex<String>> = OnceLock::new();

fn get_selected_model() -> String {
    SELECTED_MODEL
        .get_or_init(|| std::sync::Mutex::new(DEFAULT_MODEL.to_string()))
        .lock()
        .map(|g| g.clone())
        .unwrap_or_else(|_| DEFAULT_MODEL.to_string())
}

/// Setzt das ausgewaehlte Ollama-Modell
pub fn set_model(model: &str) {
    if let Ok(mut guard) = SELECTED_MODEL
        .get_or_init(|| std::sync::Mutex::new(DEFAULT_MODEL.to_string()))
        .lock()
    {
        *guard = model.to_string();
    }
}

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

async fn parse_ollama_error(response: reqwest::Response) -> String {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();

    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
        if let Some(message) = json
            .get("error")
            .and_then(|value| value.as_str())
            .or_else(|| json.get("message").and_then(|value| value.as_str()))
        {
            return format!("Ollama-Fehler ({}): {}", status, message);
        }
    }

    if body.trim().is_empty() {
        format!("Ollama-Fehler: HTTP {}", status)
    } else {
        format!("Ollama-Fehler ({}): {}", status, body.trim())
    }
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
            Self::Summary => "Du bist ein praeziser Assistent fuer die Analyse von Interviews \
                und geschäftlichen Gespraechen. Verwende ausschliesslich Informationen aus dem \
                Transkript. Erfinde keine Namen, Rollen, Motive, Beschluesse, Fristen oder \
                Zusammenhaenge. Wenn etwas unklar ist, schreibe 'unklar' oder 'nicht explizit \
                genannt'. Erstelle auf Deutsch eine kurze, belastbare Zusammenfassung mit genau \
                diesen Abschnitten:\n\
                ## Kontext\n\
                (1-2 Saetze zum Anlass oder Thema, falls erkennbar)\n\
                ## Kernaussagen\n\
                (3-5 praegnante Stichpunkte mit den wichtigsten Inhalten)\n\
                ## Ergebnis\n\
                (kurz: wichtigste Erkenntnis, Einigung oder offener Stand)",

            Self::DetailedSummary => "Du bist ein praeziser Assistent fuer die Analyse von \
                Interviews und geschäftlichen Gespraechen. Verwende ausschliesslich Informationen \
                aus dem Transkript. Erfinde keine Namen, Rollen, Motive, Beschluesse, Fristen \
                oder Zusammenhaenge. Weise Aussagen nur Personen zu, wenn das im Transkript klar \
                erkennbar ist; sonst neutral formulieren. Wenn Informationen fehlen, markiere sie \
                als 'nicht explizit genannt' oder 'unklar'. Erstelle auf Deutsch eine ausfuehrliche \
                Zusammenfassung mit dieser Struktur:\n\
                ## Kontext\n\
                ## Hauptpunkte\n\
                ## Positionen und Perspektiven\n\
                (nur wenn im Transkript erkennbar)\n\
                ## Entscheidungen oder vorlaeufige Ergebnisse\n\
                ## Offene Fragen und Unsicherheiten\n\
                ## Naechste Schritte\n\
                Schreibe praezise, sachlich und ohne Wiederholungen.",

            Self::Topics => "Du bist ein praeziser Assistent fuer die Analyse von Interviews \
                und geschäftlichen Gespraechen. Verwende ausschliesslich Informationen aus dem \
                Transkript. Extrahiere die Hauptthemen und ordne sie nach Relevanz. Gib fuer jedes \
                Thema an: \
                - Thema \
                - kurze Erklaerung in 1-2 Saetzen \
                - Einordnung: Hauptthema, Nebenthema oder offener Punkt \
                Wenn ein Thema nur angedeutet wird, markiere es als 'unklar angedeutet'.",

            Self::Actions => "Du bist ein praeziser Assistent fuer die Analyse von Interviews \
                und geschäftlichen Gespraechen. Verwende ausschliesslich Informationen aus dem \
                Transkript. Extrahiere nur explizit oder sehr klar implizit genannte Aktionspunkte, \
                To-dos und Folgeaufgaben. Gib sie als nummerierte Liste aus. Fuer jeden Punkt nutze \
                dieses Schema:\n\
                1. Aufgabe: ...\n\
                Verantwortlich: ... / nicht genannt\n\
                Frist: ... / nicht genannt\n\
                Grundlage im Gespraech: ...\n\
                Wenn etwas nicht eindeutig als Aufgabe formuliert ist, fuehre es nicht als \
                Aktionspunkt auf.",

            Self::Sentiment => "Du bist ein vorsichtiger Assistent fuer Stimmungsanalyse. \
                Verwende ausschliesslich Informationen aus dem Transkript und vermeide psychologische \
                Spekulationen. Analysiere sachlich:\n\
                1. Grundton des Gespraechs: positiv, neutral, angespannt, kritisch oder gemischt\n\
                2. Markante Veraenderungen im Tonfall, falls sprachlich erkennbar\n\
                3. Kommunikationsstil, z. B. kooperativ, defensiv, konflikthaft, loesungsorientiert\n\
                Weise Stimmungen nur Personen zu, wenn Sprecher klar erkennbar sind. Wenn das nicht \
                moeglich ist, formuliere neutral auf Gespraechsebene.",

            Self::Decisions => "Du bist ein praeziser Assistent fuer die Analyse von \
                geschäftlichen Gespraechen und Interviews. Verwende ausschliesslich Informationen \
                aus dem Transkript. Extrahiere nur explizit genannte oder eindeutig festgehaltene \
                Entscheidungen, Einigungen und Festlegungen. Gib jeden Punkt nummeriert aus mit \
                genau diesem Schema:\n\
                1. Beschluss oder Einigung: ...\n\
                Status: beschlossen / vorlaeufig / offen\n\
                Verantwortlich: ... / nicht genannt\n\
                Frist: ... / nicht genannt\n\
                Beleg im Gespraech: ...\n\
                Wenn etwas diskutiert, aber nicht entschieden wurde, gehoert es nicht in diese \
                Liste, sondern hoechstens als offener Punkt.",

            Self::Protocol => "Du bist ein praeziser Protokollfuehrer fuer Interviews und \
                geschäftliche Gespraeche. Verwende ausschliesslich Informationen aus dem Transkript. \
                Erfinde keine Teilnehmer, Rollen, Beschluesse oder Fristen. Wenn etwas nicht klar \
                ist, markiere es als 'nicht explizit genannt' oder 'unklar'. Verwende genau dieses \
                Format:\n\
                ## Protokoll\n\
                ### Kontext\n\
                (Anlass oder Ziel, falls erkennbar)\n\
                ### Teilnehmer\n\
                (nur klar erkennbare Personen oder 'nicht eindeutig erkennbar')\n\
                ### Themen\n\
                (Hauptthemen in Reihenfolge des Gespraechs)\n\
                ### Kerndiskussion\n\
                (pro Thema kurz die wesentlichen Aussagen)\n\
                ### Beschluesse oder Ergebnisse\n\
                (nur klar belegbare Punkte)\n\
                ### Aktionspunkte\n\
                (Aufgabe, verantwortlich, Frist; fehlende Angaben als 'nicht genannt')\n\
                ### Offene Fragen\n\
                (nicht geklaerte Punkte)",

            Self::Full => "Du bist ein praeziser Assistent fuer die Analyse von Interviews \
                und geschäftlichen Gespraechen. Verwende ausschliesslich Informationen aus dem \
                Transkript. Erfinde nichts. Markiere Unsicherheiten klar. Erstelle eine umfassende \
                Analyse mit genau diesen Abschnitten:\n\
                1. Kontext und Ziel des Gespraechs\n\
                2. Kurzfassung der Kernaussagen\n\
                3. Hauptthemen und Positionen\n\
                4. Aktionspunkte\n\
                5. Entscheidungen und Ergebnisse\n\
                6. Offene Fragen und Risiken\n\
                7. Kommunikationsstil / Stimmung\n\
                Schreibe sachlich, strukturiert und fuer die Nachbereitung eines Interviews oder \
                Business-Gespraechs nutzbar.",
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

    if !response.status().is_success() {
        return Err(parse_ollama_error(response).await);
    }

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

    let model = get_selected_model();
    let request_body = json!({
        "model": model,
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
    info!(
        "Ollama Request: model={}, system_prompt_len={}, user_msg_len={}",
        model,
        system_prompt.len(),
        transcript.len()
    );

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

    info!("Ollama Response Status: {}", response.status());
    if !response.status().is_success() {
        return Err(parse_ollama_error(response).await);
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Antwort konnte nicht gelesen werden: {}", e))?;

    info!(
        "Ollama Response Keys: {:?}",
        body.as_object().map(|o| o.keys().collect::<Vec<_>>())
    );

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

    let model = get_selected_model();
    let request_body = json!({
        "model": model,
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
    info!(
        "Ollama Stream Request: model={}, system_prompt_len={}, user_msg_len={}",
        model,
        system_prompt.len(),
        transcript.len()
    );

    let response = client
        .post(format!("{}/api/chat", OLLAMA_BASE_URL))
        .json(&request_body)
        .send()
        .await
        .map_err(|e| {
            error!(
                "Ollama Stream Request-Fehler: {} (connect={}, timeout={})",
                e,
                e.is_connect(),
                e.is_timeout()
            );
            if e.is_connect() {
                "Ollama ist nicht erreichbar. Bitte starte Ollama mit 'ollama serve'.".to_string()
            } else if e.is_timeout() {
                "Ollama-Anfrage hat das Zeitlimit ueberschritten.".to_string()
            } else {
                format!("Ollama-Fehler: {}", e)
            }
        })?;

    info!("Ollama Stream Response Status: {}", response.status());
    if !response.status().is_success() {
        return Err(parse_ollama_error(response).await);
    }

    let mut full_response = String::new();
    let mut pending = String::new();
    let mut response = response;

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("Streaming-Antwort konnte nicht gelesen werden: {}", e))?
    {
        pending.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(line_end) = pending.find('\n') {
            let line = pending[..line_end].trim().to_string();
            pending.drain(..=line_end);

            if line.is_empty() {
                continue;
            }

            match serde_json::from_str::<serde_json::Value>(&line) {
                Ok(json) => {
                    if let Some(content) = json["message"]["content"].as_str() {
                        full_response.push_str(content);

                        let _ = app.emit(
                            "analysis-token",
                            serde_json::json!({
                                "token": content,
                                "task": task,
                                "accumulated": full_response.len(),
                            }),
                        );
                    }

                    if json["done"].as_bool().unwrap_or(false) {
                        pending.clear();
                        break;
                    }
                }
                Err(e) => {
                    warn!("Ungueltige JSON-Zeile im Stream: {} ({})", line, e);
                }
            }
        }
    }

    let trailing = pending.trim();
    if !trailing.is_empty() {
        match serde_json::from_str::<serde_json::Value>(trailing) {
            Ok(json) => {
                if let Some(content) = json["message"]["content"].as_str() {
                    full_response.push_str(content);
                    let _ = app.emit(
                        "analysis-token",
                        serde_json::json!({
                            "token": content,
                            "task": task,
                            "accumulated": full_response.len(),
                        }),
                    );
                }
            }
            Err(e) => {
                warn!("Ungueltige JSON-Restzeile im Stream: {} ({})", trailing, e);
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
        assert!(matches!(
            AnalysisTask::from_str("summary"),
            AnalysisTask::Summary
        ));
        assert!(matches!(
            AnalysisTask::from_str("topics"),
            AnalysisTask::Topics
        ));
        assert!(matches!(
            AnalysisTask::from_str("actions"),
            AnalysisTask::Actions
        ));
        assert!(matches!(
            AnalysisTask::from_str("sentiment"),
            AnalysisTask::Sentiment
        ));
        assert!(matches!(
            AnalysisTask::from_str("decisions"),
            AnalysisTask::Decisions
        ));
        assert!(matches!(
            AnalysisTask::from_str("protocol"),
            AnalysisTask::Protocol
        ));
        assert!(matches!(AnalysisTask::from_str("full"), AnalysisTask::Full));
        // Unknown -> Summary
        assert!(matches!(
            AnalysisTask::from_str("unknown"),
            AnalysisTask::Summary
        ));
    }

    #[test]
    fn test_system_prompts_not_empty() {
        let tasks = vec![
            "summary",
            "topics",
            "actions",
            "sentiment",
            "decisions",
            "protocol",
            "full",
        ];
        for task in tasks {
            let t = AnalysisTask::from_str(task);
            assert!(
                !t.system_prompt().is_empty(),
                "Prompt fuer {} ist leer",
                task
            );
        }
    }

    #[test]
    fn test_labels() {
        assert_eq!(AnalysisTask::Summary.label(), "Zusammenfassung");
        assert_eq!(AnalysisTask::Protocol.label(), "Protokoll");
        assert_eq!(AnalysisTask::Full.label(), "Vollanalyse");
    }
}
