use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Host, SupportedStreamConfig};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};
use tracing::{error, info};

/// Globaler Aufnahme-State
pub struct RecordingState {
    pub samples: Arc<Mutex<Vec<f32>>>,
    pub is_recording: Arc<Mutex<bool>>,
    pub sample_rate: Arc<Mutex<u32>>,
    pub gain: Arc<Mutex<f32>>,
    /// Monitoring-Flag (Pegel anzeigen ohne Aufnahme)
    pub is_monitoring: Arc<AtomicBool>,
    /// Live-Transkription aktiv
    pub live_transcribing: Arc<AtomicBool>,
    /// Bisher transkribierte Samples (Offset fuer Live-Transkription)
    pub live_transcribed_offset: Arc<Mutex<usize>>,
    /// Ausgewaehltes Audio-Geraet (None = Standard)
    pub selected_device: Arc<Mutex<Option<String>>>,
}

impl RecordingState {
    pub fn new() -> Self {
        Self {
            samples: Arc::new(Mutex::new(Vec::new())),
            is_recording: Arc::new(Mutex::new(false)),
            sample_rate: Arc::new(Mutex::new(48000)),
            gain: Arc::new(Mutex::new(1.0)),
            is_monitoring: Arc::new(AtomicBool::new(false)),
            live_transcribing: Arc::new(AtomicBool::new(false)),
            live_transcribed_offset: Arc::new(Mutex::new(0)),
            selected_device: Arc::new(Mutex::new(None)),
        }
    }
}

fn resolve_input_device(host: &Host, selected_device_name: Option<&str>) -> Result<Device, String> {
    if let Some(device_name) = selected_device_name {
        host.input_devices()
            .map_err(|e| format!("Geraete-Liste Fehler: {}", e))?
            .find(|device| {
                device
                    .name()
                    .map(|name| name == device_name)
                    .unwrap_or(false)
            })
            .ok_or_else(|| format!("Audio-Geraet '{}' nicht gefunden", device_name))
    } else {
        host.default_input_device()
            .ok_or("Kein Mikrofon gefunden. Bitte ein Mikrofon anschliessen.".to_string())
    }
}

fn preferred_input_config(device: &Device) -> Result<SupportedStreamConfig, String> {
    device
        .supported_input_configs()
        .map_err(|e| format!("Audio-Config-Fehler: {}", e))?
        .find(|config| config.min_sample_rate().0 <= 48000)
        .or_else(|| device.supported_input_configs().ok()?.next())
        .map(|config| config.with_max_sample_rate())
        .ok_or("Keine unterstuetzte Audio-Konfiguration gefunden".to_string())
}

fn downmix_to_mono(data: &[f32], channels: usize, gain: f32) -> Vec<f32> {
    let mut mono = Vec::with_capacity(data.len() / channels.max(1));
    for chunk in data.chunks(channels.max(1)) {
        let avg: f32 = chunk.iter().sum::<f32>() / chunk.len() as f32;
        mono.push(avg * gain);
    }
    mono
}

fn normalized_audio_level(samples: &[f32]) -> f32 {
    let rms = if samples.is_empty() {
        0.0
    } else {
        let sum_sq: f32 = samples.iter().map(|sample| sample * sample).sum();
        (sum_sq / samples.len() as f32).sqrt()
    };
    let db = if rms > 0.0 { 20.0 * rms.log10() } else { -60.0 };
    (db.clamp(-60.0, 0.0) + 60.0) / 60.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordingOutputFormat {
    Wav,
    Mp3,
    M4a,
}

impl RecordingOutputFormat {
    fn from_str(value: &str) -> Result<Self, String> {
        match value.to_ascii_lowercase().as_str() {
            "wav" => Ok(Self::Wav),
            "mp3" => Ok(Self::Mp3),
            "m4a" => Ok(Self::M4a),
            other => Err(format!("Unbekanntes Aufnahmeformat: {}", other)),
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
            Self::M4a => "m4a",
        }
    }
}

fn convert_wav_to_format(
    input_wav: &std::path::Path,
    output_path: &std::path::Path,
    format: RecordingOutputFormat,
) -> Result<(), String> {
    if format == RecordingOutputFormat::Wav {
        return Ok(());
    }

    let mut command = Command::new("ffmpeg");
    command.arg("-y").arg("-i").arg(input_wav);

    match format {
        RecordingOutputFormat::Mp3 => {
            command.args(["-codec:a", "libmp3lame", "-b:a", "192k"]);
        }
        RecordingOutputFormat::M4a => {
            command.args(["-codec:a", "aac", "-b:a", "192k"]);
        }
        RecordingOutputFormat::Wav => {}
    }

    let output = command.arg(output_path).output().map_err(|e| {
        format!(
            "Konnte ffmpeg fuer {} nicht starten: {}",
            format.extension(),
            e
        )
    })?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "Audio-Konvertierung nach {} fehlgeschlagen: {}",
            format.extension(),
            stderr.trim()
        ))
    }
}

/// Listet alle verfuegbaren Audio-Eingabegeraete auf
#[tauri::command]
pub async fn list_audio_devices() -> Result<Vec<serde_json::Value>, String> {
    let host = cpal::default_host();
    let mut devices = Vec::new();

    let default_name = host.default_input_device().and_then(|d| d.name().ok());

    if let Some(input_devices) = host.input_devices().ok() {
        for device in input_devices {
            let name = device.name().unwrap_or_else(|_| "Unbekannt".to_string());
            let is_default = default_name.as_ref().map(|dn| dn == &name).unwrap_or(false);

            // Versuche Sample-Rate-Info zu bekommen
            let sample_rates: Vec<u32> = device
                .supported_input_configs()
                .ok()
                .map(|configs| {
                    let mut rates: Vec<u32> = configs.map(|c| c.min_sample_rate().0).collect();
                    rates.sort();
                    rates.dedup();
                    rates
                })
                .unwrap_or_default();

            devices.push(serde_json::json!({
                "name": name,
                "is_default": is_default,
                "sample_rates": sample_rates,
            }));
        }
    }

    Ok(devices)
}

/// Setzt das ausgewaehlte Audio-Geraet
#[tauri::command]
pub async fn set_audio_device(
    state: tauri::State<'_, Arc<Mutex<RecordingState>>>,
    device_name: Option<String>,
) -> Result<(), String> {
    let inner = state.lock().map_err(|e| e.to_string())?;
    let mut dev = inner.selected_device.lock().map_err(|e| e.to_string())?;
    *dev = device_name;
    Ok(())
}

/// Setzt die Aufnahme-Verstaerkung (Gain)
#[tauri::command]
pub async fn set_recording_gain(
    state: tauri::State<'_, Arc<Mutex<RecordingState>>>,
    gain: f32,
) -> Result<(), String> {
    let clamped = gain.clamp(0.1, 10.0);
    let inner = state.lock().map_err(|e| e.to_string())?;
    let mut g = inner.gain.lock().map_err(|e| e.to_string())?;
    *g = clamped;
    info!("Aufnahme-Gain gesetzt auf: {:.1}x", clamped);
    Ok(())
}

/// Liest den aktuellen Gain-Wert
#[tauri::command]
pub async fn get_recording_gain(
    state: tauri::State<'_, Arc<Mutex<RecordingState>>>,
) -> Result<f32, String> {
    let inner = state.lock().map_err(|e| e.to_string())?;
    let g = inner.gain.lock().map_err(|e| e.to_string())?;
    Ok(*g)
}

/// Startet Audio-Monitoring (Pegel anzeigen ohne Aufnahme)
#[tauri::command]
pub async fn start_monitoring(
    app: AppHandle,
    state: tauri::State<'_, Arc<Mutex<RecordingState>>>,
) -> Result<String, String> {
    // Nicht starten wenn bereits aufgenommen wird
    {
        let inner = state.lock().map_err(|e| e.to_string())?;
        let is_rec = inner.is_recording.lock().map_err(|e| e.to_string())?;
        if *is_rec {
            return Err("Aufnahme laeuft gerade".to_string());
        }
    }

    // Nicht doppelt starten
    {
        let inner = state.lock().map_err(|e| e.to_string())?;
        if inner.is_monitoring.load(Ordering::Relaxed) {
            return Ok("Monitoring laeuft bereits".to_string());
        }
    }

    let (gain_arc, is_monitoring, selected_device_name) = {
        let inner = state.lock().map_err(|e| e.to_string())?;
        let selected_device_name = inner
            .selected_device
            .lock()
            .map_err(|e| e.to_string())?
            .clone();
        (
            inner.gain.clone(),
            inner.is_monitoring.clone(),
            selected_device_name,
        )
    };
    is_monitoring.store(true, Ordering::Relaxed);

    let app_handle = app.clone();

    std::thread::spawn(move || {
        let host = cpal::default_host();
        let device = match resolve_input_device(&host, selected_device_name.as_deref()) {
            Ok(device) => device,
            Err(err) => {
                error!("{}", err);
                is_monitoring.store(false, Ordering::Relaxed);
                return;
            }
        };

        let supported_config = match preferred_input_config(&device) {
            Ok(config) => config,
            Err(err) => {
                error!("{}", err);
                is_monitoring.store(false, Ordering::Relaxed);
                return;
            }
        };

        let sr = supported_config.sample_rate().0;
        let channels = supported_config.channels() as usize;

        let last_level: Arc<Mutex<f32>> = Arc::new(Mutex::new(0.0));
        let last_level_clone = last_level.clone();
        let gain_clone = gain_arc.clone();

        let err_fn = |err: cpal::StreamError| {
            error!("Monitoring-Stream-Fehler: {}", err);
        };

        let stream = match device.build_input_stream(
            &supported_config.config(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let current_gain = *gain_clone.lock().unwrap();
                let mono = downmix_to_mono(data, channels, current_gain);
                *last_level_clone.lock().unwrap() = normalized_audio_level(&mono);
            },
            err_fn,
            None,
        ) {
            Ok(s) => s,
            Err(e) => {
                error!("Monitoring-Stream konnte nicht gestartet werden: {}", e);
                is_monitoring.store(false, Ordering::Relaxed);
                return;
            }
        };

        if let Err(e) = stream.play() {
            error!("Monitoring-Play-Fehler: {}", e);
            is_monitoring.store(false, Ordering::Relaxed);
            return;
        }

        info!(
            "Audio-Monitoring gestartet ({} Hz, {} Kanaele)",
            sr, channels
        );

        // Level-Events senden solange monitoring aktiv
        loop {
            std::thread::sleep(std::time::Duration::from_millis(80));
            if !is_monitoring.load(Ordering::Relaxed) {
                break;
            }

            let level = *last_level.lock().unwrap();
            let _ = app_handle.emit(
                "audio-level",
                serde_json::json!({
                    "level": level,
                    "db": level * 60.0 - 60.0,
                }),
            );
        }

        drop(stream);
        info!("Audio-Monitoring beendet");
    });

    Ok("Monitoring gestartet".to_string())
}

/// Stoppt Audio-Monitoring
#[tauri::command]
pub async fn stop_monitoring(
    state: tauri::State<'_, Arc<Mutex<RecordingState>>>,
) -> Result<(), String> {
    let inner = state.lock().map_err(|e| e.to_string())?;
    inner.is_monitoring.store(false, Ordering::Relaxed);
    Ok(())
}

/// Startet Live-Transkription waehrend der Aufnahme
/// Transkribiert alle ~10 Sekunden den aktuellen Sample-Buffer
#[tauri::command]
pub async fn start_live_transcription(
    app: AppHandle,
    rec_state: tauri::State<'_, Arc<Mutex<RecordingState>>>,
    whisper_state: tauri::State<'_, Arc<Mutex<crate::whisper::WhisperState>>>,
    language: Option<String>,
) -> Result<(), String> {
    let lang = language.unwrap_or_else(|| "de".to_string());

    // Pruefen ob aufgenommen wird
    {
        let inner = rec_state.lock().map_err(|e| e.to_string())?;
        let is_rec = inner.is_recording.lock().map_err(|e| e.to_string())?;
        if !*is_rec {
            return Err("Aufnahme muss laufen bevor Live-Transkription gestartet wird".to_string());
        }
    }

    // Nicht doppelt starten
    {
        let inner = rec_state.lock().map_err(|e| e.to_string())?;
        if inner.live_transcribing.load(Ordering::Relaxed) {
            return Ok(()); // Bereits aktiv
        }
    }

    // Live-Transkription aktivieren + Offset zuruecksetzen
    {
        let inner = rec_state.lock().map_err(|e| e.to_string())?;
        inner.live_transcribing.store(true, Ordering::Relaxed);
        let mut offset = inner
            .live_transcribed_offset
            .lock()
            .map_err(|e| e.to_string())?;
        *offset = 0;
    }

    // Whisper-Modell laden (falls noch nicht geschehen)
    {
        let mut ws = whisper_state.lock().map_err(|e| e.to_string())?;
        let model_path = crate::whisper::default_model_path();
        ws.load_model(&model_path)?;
    }

    // Arcs klonen fuer den Thread
    let samples_arc = {
        let inner = rec_state.lock().map_err(|e| e.to_string())?;
        inner.samples.clone()
    };
    let sample_rate_arc = {
        let inner = rec_state.lock().map_err(|e| e.to_string())?;
        inner.sample_rate.clone()
    };
    let live_active = {
        let inner = rec_state.lock().map_err(|e| e.to_string())?;
        inner.live_transcribing.clone()
    };
    let live_offset = {
        let inner = rec_state.lock().map_err(|e| e.to_string())?;
        inner.live_transcribed_offset.clone()
    };
    let is_recording_arc = {
        let inner = rec_state.lock().map_err(|e| e.to_string())?;
        inner.is_recording.clone()
    };
    let whisper_arc = whisper_state.inner().clone();
    let app_handle = app.clone();

    std::thread::spawn(move || {
        info!("Live-Transkription gestartet (Sprache: {})", lang);

        // Status-Event: Start
        let _ = app_handle.emit(
            "live-transcription-status",
            serde_json::json!({
                "status": "loading",
                "message": "Whisper-Modell wird geladen...",
            }),
        );

        let mut accumulated_text = String::new();
        let mut chunk_count = 0u32;

        loop {
            // 3 Sekunden warten
            std::thread::sleep(std::time::Duration::from_secs(3));

            if !live_active.load(Ordering::Relaxed) {
                break;
            }

            // Pruefen ob Aufnahme noch laeuft
            let still_recording = {
                let flag = is_recording_arc.lock().unwrap();
                *flag
            };

            // Aktuelle Samples holen
            let new_samples = {
                let all_samples = samples_arc.lock().unwrap();
                let current_offset = *live_offset.lock().unwrap();

                if all_samples.len() <= current_offset {
                    if !still_recording {
                        break;
                    }
                    // Status: Warte auf Audio
                    let _ = app_handle.emit(
                        "live-transcription-status",
                        serde_json::json!({
                            "status": "waiting",
                            "message": "Warte auf Audio-Daten...",
                        }),
                    );
                    continue;
                }

                let new_data = all_samples[current_offset..].to_vec();
                *live_offset.lock().unwrap() = all_samples.len();
                new_data
            };

            if new_samples.is_empty() {
                if !still_recording {
                    break;
                }
                continue;
            }

            let sr = *sample_rate_arc.lock().unwrap();
            let duration_secs = new_samples.len() as f64 / sr as f64;

            // Mindestens 1.5 Sekunden Audio
            if duration_secs < 1.5 {
                let all_samples = samples_arc.lock().unwrap();
                *live_offset.lock().unwrap() = all_samples.len() - new_samples.len();
                if !still_recording {
                    break;
                }
                continue;
            }

            chunk_count += 1;
            info!(
                "Live-Transkription: {:.1}s neue Audio-Daten ({} Hz)",
                duration_secs, sr
            );

            // Status-Event: Transkribiere
            let _ = app_handle.emit(
                "live-transcription-status",
                serde_json::json!({
                    "status": "transcribing",
                    "message": format!("Transkribiere Chunk #{} ({:.1}s Audio)...", chunk_count, duration_secs),
                    "chunk": chunk_count,
                    "duration_secs": duration_secs,
                }),
            );

            // Auf 16kHz resamplen
            let samples_16k = if sr != 16000 {
                resample_simple(&new_samples, sr, 16000)
            } else {
                new_samples
            };

            // Whisper transkribieren
            let transcript_result = {
                let ws = match whisper_arc.lock() {
                    Ok(guard) => guard,
                    Err(e) => {
                        error!("Whisper-Lock-Fehler: {}", e);
                        continue;
                    }
                };
                ws.transcribe_chunk(&samples_16k, Some(&lang))
            };

            match transcript_result {
                Ok(text) => {
                    if !text.trim().is_empty() {
                        if accumulated_text.is_empty() {
                            accumulated_text = text.clone();
                        } else {
                            accumulated_text = format!("{} {}", accumulated_text, text);
                        }

                        info!(
                            "Live-Transkription Chunk: \"{}\" (gesamt: {} Zeichen)",
                            if text.len() > 60 { &text[..60] } else { &text },
                            accumulated_text.len()
                        );

                        // Status-Event: Ergebnis
                        let _ = app_handle.emit(
                            "live-transcription-status",
                            serde_json::json!({
                                "status": "result",
                                "message": format!("Chunk #{} transkribiert ({} Zeichen)", chunk_count, accumulated_text.len()),
                                "chunk": chunk_count,
                                "total_chars": accumulated_text.len(),
                            }),
                        );

                        // Event an Frontend senden
                        let _ = app_handle.emit(
                            "live-transcription",
                            serde_json::json!({
                                "chunk_text": text,
                                "accumulated_text": accumulated_text,
                                "duration_secs": duration_secs,
                            }),
                        );
                    } else {
                        // Leeres Ergebnis
                        let _ = app_handle.emit(
                            "live-transcription-status",
                            serde_json::json!({
                                "status": "empty",
                                "message": format!("Chunk #{} - keine Sprache erkannt", chunk_count),
                                "chunk": chunk_count,
                            }),
                        );
                    }
                }
                Err(e) => {
                    error!("Live-Transkription Fehler: {}", e);
                    let _ = app_handle.emit(
                        "live-transcription-status",
                        serde_json::json!({
                            "status": "error",
                            "message": format!("Fehler bei Chunk #{}: {}", chunk_count, e),
                        }),
                    );
                }
            }
        }

        live_active.store(false, Ordering::Relaxed);

        // Status-Event: Beendet
        let _ = app_handle.emit(
            "live-transcription-status",
            serde_json::json!({
                "status": "done",
                "message": format!("Transkription beendet ({} Zeichen)", accumulated_text.len()),
                "total_chars": accumulated_text.len(),
            }),
        );

        info!(
            "Live-Transkription beendet (gesamt: {} Zeichen)",
            accumulated_text.len()
        );
    });

    Ok(())
}

/// Startet die Audio-Aufnahme vom Standard-Mikrofon
#[tauri::command]
pub async fn start_recording(
    app: AppHandle,
    state: tauri::State<'_, Arc<Mutex<RecordingState>>>,
) -> Result<String, String> {
    // Monitoring stoppen falls aktiv
    {
        let inner = state.lock().map_err(|e| e.to_string())?;
        inner.is_monitoring.store(false, Ordering::Relaxed);
    }
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Pruefen ob schon aufgenommen wird
    {
        let inner = state.lock().map_err(|e| e.to_string())?;
        let is_rec = inner.is_recording.lock().map_err(|e| e.to_string())?;
        if *is_rec {
            return Err("Aufnahme laeuft bereits".to_string());
        }
    }

    // Alte Samples loeschen
    {
        let inner = state.lock().map_err(|e| e.to_string())?;
        let mut samples = inner.samples.lock().map_err(|e| e.to_string())?;
        samples.clear();
    }

    let (samples, is_recording, sample_rate_arc, gain_arc, selected_device_name) = {
        let inner = state.lock().map_err(|e| e.to_string())?;
        let dev_name = inner
            .selected_device
            .lock()
            .map_err(|e| e.to_string())?
            .clone();
        (
            inner.samples.clone(),
            inner.is_recording.clone(),
            inner.sample_rate.clone(),
            inner.gain.clone(),
            dev_name,
        )
    };

    let app_handle = app.clone();
    let (startup_tx, startup_rx) = std::sync::mpsc::channel();

    // In einem eigenen Thread starten (cpal::Stream ist nicht Send)
    let _handle = std::thread::spawn(move || -> Result<String, String> {
        let result = (|| -> Result<String, String> {
            let host = cpal::default_host();

            let device = resolve_input_device(&host, selected_device_name.as_deref())?;

            let device_name = device.name().unwrap_or_else(|_| "Unbekannt".to_string());

            let supported_config = preferred_input_config(&device)?;
            let sr = supported_config.sample_rate().0;
            let channels = supported_config.channels() as usize;

            {
                let mut sr_lock = sample_rate_arc.lock().map_err(|e| e.to_string())?;
                *sr_lock = sr;
            }

            info!(
                "Aufnahme gestartet: {} ({} Hz, {} Kanaele)",
                device_name, sr, channels
            );

            let samples_clone = samples.clone();
            let is_recording_clone = is_recording.clone();
            let gain_clone = gain_arc.clone();
            let app_for_level = app_handle.clone();

            let err_fn = |err: cpal::StreamError| {
                error!("Audio-Stream-Fehler: {}", err);
            };

            // Level-Messung: letzten RMS-Wert speichern
            let last_level: Arc<Mutex<f32>> = Arc::new(Mutex::new(0.0));
            let last_level_clone = last_level.clone();

            let stream = device
                .build_input_stream(
                    &supported_config.config(),
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        let current_gain = *gain_clone.lock().unwrap();
                        let mono = downmix_to_mono(data, channels, current_gain);
                        *last_level_clone.lock().unwrap() = normalized_audio_level(&mono);

                        // Nur Samples speichern wenn aufgenommen wird
                        let should_record = *is_recording_clone.lock().unwrap();
                        if should_record {
                            let mut buf = samples_clone.lock().unwrap();
                            buf.extend_from_slice(&mono);
                        }
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| format!("Audio-Stream konnte nicht gestartet werden: {}", e))?;

            stream
                .play()
                .map_err(|e| format!("Audio-Play-Fehler: {}", e))?;

            // Flag setzen
            {
                let mut flag = is_recording.lock().map_err(|e| e.to_string())?;
                *flag = true;
            }
            let _ = startup_tx.send(Ok(()));

            // Level-Events an Frontend senden (alle 80ms)
            let level_for_emit = last_level.clone();
            let app_emit = app_for_level.clone();
            let is_rec_for_level = is_recording.clone();

            let _level_thread = std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(80));
                let flag = is_rec_for_level.lock().unwrap();
                if !*flag {
                    break;
                }
                drop(flag);

                let level = *level_for_emit.lock().unwrap();
                let _ = app_emit.emit(
                    "audio-level",
                    serde_json::json!({
                        "level": level,
                        "db": level * 60.0 - 60.0,
                    }),
                );
            });

            // Stream am Leben halten bis is_recording = false
            loop {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let flag = is_recording.lock().unwrap();
                if !*flag {
                    break;
                }
            }

            // Stream wird hier gedroppt
            drop(stream);
            info!("Aufnahme-Thread beendet");

            Ok(device_name)
        })();

        if let Err(err) = &result {
            let _ = startup_tx.send(Err(err.clone()));
        }

        result
    });

    match startup_rx.recv_timeout(std::time::Duration::from_secs(2)) {
        Ok(Ok(())) => {}
        Ok(Err(err)) => return Err(err),
        Err(_) => {
            return Err(
                "Mikrofonstart hat keine Rueckmeldung geliefert. Bitte Audio-Geraet pruefen."
                    .to_string(),
            )
        }
    }

    let _ = app;

    Ok("Mikrofon aktiv".to_string())
}

/// Stoppt die Aufnahme und speichert das Ergebnis als WAV (16kHz fuer kleinere Dateien)
#[tauri::command]
pub async fn stop_recording(
    app: AppHandle,
    state: tauri::State<'_, Arc<Mutex<RecordingState>>>,
    format: Option<String>,
) -> Result<serde_json::Value, String> {
    // Flag setzen -> Thread beendet sich selbst
    {
        let inner = state.lock().map_err(|e| e.to_string())?;
        let mut flag = inner.is_recording.lock().map_err(|e| e.to_string())?;
        *flag = false;
    }

    // Live-Transkription NICHT sofort stoppen - der Thread soll die letzte
    // Transkription noch fertigstellen. Der Thread beendet sich selbst,
    // wenn is_recording=false und keine neuen Samples mehr kommen.

    // Kurz warten bis Thread beendet
    std::thread::sleep(std::time::Duration::from_millis(300));

    let (samples, sample_rate) = {
        let inner = state.lock().map_err(|e| e.to_string())?;
        let s = inner.samples.lock().map_err(|e| e.to_string())?.clone();
        let sr = *inner.sample_rate.lock().map_err(|e| e.to_string())?;
        (s, sr)
    };

    if samples.is_empty() {
        return Err("Aufnahme enthaelt keine Samples. Mikrofon moeglicherweise nicht verbunden oder Lautstaerke auf 0.".to_string());
    }

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    info!(
        "Aufnahme gestoppt: {:.1}s, {} Samples @ {} Hz",
        duration_secs,
        samples.len(),
        sample_rate
    );

    // Auf 16kHz resamplen fuer kleinere Dateien (Whisper braucht sowieso 16kHz)
    let (save_samples, save_rate) = if sample_rate != 16000 {
        info!("Resample fuer Speicherung: {} Hz -> 16000 Hz", sample_rate);
        let resampled = resample_simple(&samples, sample_rate, 16000);
        (resampled, 16000u32)
    } else {
        (samples, sample_rate)
    };
    let output_format = RecordingOutputFormat::from_str(format.as_deref().unwrap_or("wav"))?;

    // Als WAV speichern
    let recordings_dir = std::path::Path::new("recordings");
    if !recordings_dir.exists() {
        std::fs::create_dir_all(recordings_dir)
            .map_err(|e| format!("Konnte recordings-Verzeichnis nicht erstellen: {}", e))?;
    }

    let now = chrono::Local::now();
    let date_folder = now.format("%Y-%m-%d").to_string();
    let session_dir = recordings_dir.join(&date_folder);
    std::fs::create_dir_all(&session_dir)
        .map_err(|e| format!("Konnte Aufnahmeordner nicht erstellen: {}", e))?;

    let timestamp = now.format("%Y%m%d_%H%M%S");
    let base_name = format!("aufnahme_{}", timestamp);
    let wav_path = session_dir.join(format!("{}.wav", base_name));

    write_wav(&save_samples, save_rate, &wav_path)?;

    let path = if output_format == RecordingOutputFormat::Wav {
        wav_path.clone()
    } else {
        let converted_path =
            session_dir.join(format!("{}.{}", base_name, output_format.extension()));
        convert_wav_to_format(&wav_path, &converted_path, output_format)?;
        std::fs::remove_file(&wav_path)
            .map_err(|e| format!("Temporäre WAV-Datei konnte nicht gelöscht werden: {}", e))?;
        converted_path
    };

    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();

    let file_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let path_str = path.to_string_lossy().to_string();
    info!(
        "Aufnahme gespeichert: {} ({:.1}s, {} Bytes)",
        path_str, duration_secs, file_size
    );

    let _ = app.emit(
        "recording-stopped",
        serde_json::json!({
            "path": path_str,
            "session_dir": session_dir.to_string_lossy().to_string(),
            "duration_secs": duration_secs,
            "filename": filename,
            "file_size": file_size,
        }),
    );

    // Monitoring nach Stop automatisch wieder starten
    let (is_monitoring, gain_arc, selected_device_name) = {
        let inner = state.lock().map_err(|e| e.to_string())?;
        let selected_device_name = inner
            .selected_device
            .lock()
            .map_err(|e| e.to_string())?
            .clone();
        (
            inner.is_monitoring.clone(),
            inner.gain.clone(),
            selected_device_name,
        )
    };
    let app_handle = app.clone();
    is_monitoring.store(true, Ordering::Relaxed);

    std::thread::spawn(move || {
        // Kurz warten bis Aufnahme-Thread beendet ist
        std::thread::sleep(std::time::Duration::from_millis(500));

        let host = cpal::default_host();
        let device = match resolve_input_device(&host, selected_device_name.as_deref()) {
            Ok(device) => device,
            Err(_) => return,
        };

        let supported_config = match preferred_input_config(&device) {
            Ok(config) => config,
            Err(_) => return,
        };

        let channels = supported_config.channels() as usize;

        let last_level: Arc<Mutex<f32>> = Arc::new(Mutex::new(0.0));
        let last_level_clone = last_level.clone();
        let gain_clone = gain_arc.clone();

        let err_fn = |err: cpal::StreamError| {
            error!("Monitoring-Stream-Fehler: {}", err);
        };

        let stream = match device.build_input_stream(
            &supported_config.config(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let current_gain = *gain_clone.lock().unwrap();
                let mono = downmix_to_mono(data, channels, current_gain);
                *last_level_clone.lock().unwrap() = normalized_audio_level(&mono);
            },
            err_fn,
            None,
        ) {
            Ok(s) => s,
            Err(_) => return,
        };

        if stream.play().is_err() {
            return;
        }

        info!("Monitoring nach Aufnahme automatisch gestartet");

        loop {
            std::thread::sleep(std::time::Duration::from_millis(80));
            if !is_monitoring.load(Ordering::Relaxed) {
                break;
            }
            let level = *last_level.lock().unwrap();
            let _ = app_handle.emit(
                "audio-level",
                serde_json::json!({
                    "level": level,
                    "db": level * 60.0 - 60.0,
                }),
            );
        }

        drop(stream);
        info!("Auto-Monitoring beendet");
    });

    Ok(serde_json::json!({
        "path": path_str,
        "session_dir": session_dir.to_string_lossy().to_string(),
        "duration_secs": duration_secs,
        "filename": filename,
        "sample_count": save_samples.len(),
        "file_size": file_size,
        "sample_rate": save_rate,
    }))
}

/// Einfaches Resampling mit linearer Interpolation
fn resample_simple(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate {
        return samples.to_vec();
    }
    let ratio = to_rate as f64 / from_rate as f64;
    let new_len = (samples.len() as f64 * ratio) as usize;
    let mut output = Vec::with_capacity(new_len);

    for i in 0..new_len {
        let src_pos = i as f64 / ratio;
        let src_idx = src_pos as usize;
        let frac = src_pos - src_idx as f64;

        let s0 = samples[src_idx];
        let s1 = if src_idx + 1 < samples.len() {
            samples[src_idx + 1]
        } else {
            s0
        };

        output.push(s0 + (s1 - s0) * frac as f32);
    }

    output
}

/// Schreibt 16-bit PCM Mono WAV
fn write_wav(samples: &[f32], sample_rate: u32, path: &std::path::Path) -> Result<(), String> {
    let num_samples = samples.len();
    let data_size = (num_samples * 2) as u32;

    let mut file = std::fs::File::create(path)
        .map_err(|e| format!("Konnte WAV-Datei nicht erstellen: {}", e))?;

    use std::io::Write;

    file.write_all(b"RIFF").map_err(|e| e.to_string())?;
    file.write_all(&(36 + data_size).to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(b"WAVE").map_err(|e| e.to_string())?;
    file.write_all(b"fmt ").map_err(|e| e.to_string())?;
    file.write_all(&16u32.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(&1u16.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(&1u16.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(&sample_rate.to_le_bytes())
        .map_err(|e| e.to_string())?;
    let byte_rate = sample_rate * 2;
    file.write_all(&byte_rate.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(&2u16.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(&16u16.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(b"data").map_err(|e| e.to_string())?;
    file.write_all(&data_size.to_le_bytes())
        .map_err(|e| e.to_string())?;

    for &sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let pcm = if clamped < 0.0 {
            (clamped * 32768.0) as i16
        } else {
            (clamped * 32767.0) as i16
        };
        file.write_all(&pcm.to_le_bytes())
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recording_output_format_parsing() {
        assert_eq!(
            RecordingOutputFormat::from_str("wav").unwrap(),
            RecordingOutputFormat::Wav
        );
        assert_eq!(
            RecordingOutputFormat::from_str("mp3").unwrap(),
            RecordingOutputFormat::Mp3
        );
        assert_eq!(
            RecordingOutputFormat::from_str("m4a").unwrap(),
            RecordingOutputFormat::M4a
        );
        assert!(RecordingOutputFormat::from_str("flac").is_err());
    }
}
