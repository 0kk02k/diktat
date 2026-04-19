use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
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
}

impl RecordingState {
    pub fn new() -> Self {
        Self {
            samples: Arc::new(Mutex::new(Vec::new())),
            is_recording: Arc::new(Mutex::new(false)),
            sample_rate: Arc::new(Mutex::new(48000)),
            gain: Arc::new(Mutex::new(1.0)),
            is_monitoring: Arc::new(AtomicBool::new(false)),
        }
    }
}

// Send+Sync: wir speichern den Stream nicht hier
unsafe impl Send for RecordingState {}
unsafe impl Sync for RecordingState {}

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

    let (gain_arc, is_monitoring) = {
        let inner = state.lock().map_err(|e| e.to_string())?;
        (inner.gain.clone(), inner.is_monitoring.clone())
    };
    is_monitoring.store(true, Ordering::Relaxed);

    let app_handle = app.clone();

    std::thread::spawn(move || {
        let host = cpal::default_host();
        let device = match host.default_input_device() {
            Some(d) => d,
            None => {
                error!("Kein Mikrofon fuer Monitoring gefunden");
                is_monitoring.store(false, Ordering::Relaxed);
                return;
            }
        };

        let supported_config = device
            .supported_input_configs()
            .ok()
            .and_then(|mut configs| configs.find(|c| c.min_sample_rate().0 <= 48000))
            .or_else(|| {
                device.supported_input_configs().ok()?.next()
            });

        let supported_config = match supported_config {
            Some(c) => c,
            None => {
                error!("Keine Audio-Config fuer Monitoring gefunden");
                is_monitoring.store(false, Ordering::Relaxed);
                return;
            }
        };

        let config = supported_config.with_max_sample_rate();
        let sr = config.sample_rate().0;
        let channels = config.channels() as usize;

        let last_level: Arc<Mutex<f32>> = Arc::new(Mutex::new(0.0));
        let last_level_clone = last_level.clone();
        let gain_clone = gain_arc.clone();

        let err_fn = |err: cpal::StreamError| {
            error!("Monitoring-Stream-Fehler: {}", err);
        };

        let stream = match device.build_input_stream(
            &config.into(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                // Gain anwenden
                let current_gain = *gain_clone.lock().unwrap();

                // Mono-Downmix mit Gain
                let mut mono = Vec::with_capacity(data.len() / channels);
                for chunk in data.chunks(channels) {
                    let avg: f32 = chunk.iter().sum::<f32>() / channels as f32;
                    mono.push(avg * current_gain);
                }

                // RMS-Level berechnen
                let rms = if mono.is_empty() {
                    0.0
                } else {
                    let sum_sq: f32 = mono.iter().map(|s| s * s).sum();
                    (sum_sq / mono.len() as f32).sqrt()
                };
                let db = if rms > 0.0 { 20.0 * rms.log10() } else { -60.0 };
                let db_clamped = db.clamp(-60.0, 0.0);
                let level_normalized = (db_clamped + 60.0) / 60.0;

                {
                    let mut lvl = last_level_clone.lock().unwrap();
                    *lvl = level_normalized;
                }
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

        info!("Audio-Monitoring gestartet ({} Hz, {} Kanaele)", sr, channels);

        // Level-Events senden solange monitoring aktiv
        loop {
            std::thread::sleep(std::time::Duration::from_millis(80));
            if !is_monitoring.load(Ordering::Relaxed) {
                break;
            }

            let level = *last_level.lock().unwrap();
            let _ = app_handle.emit("audio-level", serde_json::json!({
                "level": level,
                "db": level * 60.0 - 60.0,
            }));
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

    let (samples, is_recording, sample_rate_arc, gain_arc) = {
        let inner = state.lock().map_err(|e| e.to_string())?;
        (
            inner.samples.clone(),
            inner.is_recording.clone(),
            inner.sample_rate.clone(),
            inner.gain.clone(),
        )
    };

    let app_handle = app.clone();

    // In einem eigenen Thread starten (cpal::Stream ist nicht Send)
    let _handle = std::thread::spawn(move || -> Result<String, String> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or("Kein Mikrofon gefunden. Bitte ein Mikrofon anschliessen.")?;

        let device_name = device
            .name()
            .unwrap_or_else(|_| "Unbekannt".to_string());

        let supported_config = device
            .supported_input_configs()
            .map_err(|e| format!("Audio-Config-Fehler: {}", e))?
            .find(|c| {
                let sr = c.min_sample_rate().0;
                sr <= 48000
            })
            .or_else(|| {
                device.supported_input_configs().ok()?.next()
            })
            .ok_or("Keine unterstuetzte Audio-Konfiguration gefunden")?;

        let config = supported_config.with_max_sample_rate();
        let sr = config.sample_rate().0;
        let channels = config.channels() as usize;

        {
            let mut sr_lock = sample_rate_arc.lock().map_err(|e| e.to_string())?;
            *sr_lock = sr;
        }

        info!("Aufnahme gestartet: {} ({} Hz, {} Kanaele)", device_name, sr, channels);

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

        let stream = device.build_input_stream(
            &config.into(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                // Gain anwenden
                let current_gain = *gain_clone.lock().unwrap();

                // Mono-Downmix mit Gain
                let mut mono = Vec::with_capacity(data.len() / channels);
                for chunk in data.chunks(channels) {
                    let avg: f32 = chunk.iter().sum::<f32>() / channels as f32;
                    mono.push(avg * current_gain);
                }

                // RMS-Level berechnen (immer, auch wenn nicht aufgenommen wird)
                let rms = if mono.is_empty() {
                    0.0
                } else {
                    let sum_sq: f32 = mono.iter().map(|s| s * s).sum();
                    (sum_sq / mono.len() as f32).sqrt()
                };
                let db = if rms > 0.0 { 20.0 * rms.log10() } else { -60.0 };
                let db_clamped = db.clamp(-60.0, 0.0);
                let level_normalized = (db_clamped + 60.0) / 60.0;

                {
                    let mut lvl = last_level_clone.lock().unwrap();
                    *lvl = level_normalized;
                }

                // Nur Samples speichern wenn aufgenommen wird
                let should_record = *is_recording_clone.lock().unwrap();
                if should_record {
                    let mut buf = samples_clone.lock().unwrap();
                    buf.extend_from_slice(&mono);
                }
            },
            err_fn,
            None,
        ).map_err(|e| format!("Audio-Stream konnte nicht gestartet werden: {}", e))?;

        stream.play().map_err(|e| format!("Audio-Play-Fehler: {}", e))?;

        // Flag setzen
        {
            let mut flag = is_recording.lock().map_err(|e| e.to_string())?;
            *flag = true;
        }

        // Level-Events an Frontend senden (alle 80ms)
        let level_for_emit = last_level.clone();
        let app_emit = app_for_level.clone();
        let is_rec_for_level = is_recording.clone();

        let _level_thread = std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(80));
                let flag = is_rec_for_level.lock().unwrap();
                if !*flag {
                    break;
                }
                drop(flag);

                let level = *level_for_emit.lock().unwrap();
                let _ = app_emit.emit("audio-level", serde_json::json!({
                    "level": level,
                    "db": level * 60.0 - 60.0,
                }));
            }
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
    });

    // Kurz warten um zu pruefen ob der Start erfolgreich war
    std::thread::sleep(std::time::Duration::from_millis(300));

    {
        let inner = state.lock().map_err(|e| e.to_string())?;
        let is_rec = inner.is_recording.lock().map_err(|e| e.to_string())?;
        if !*is_rec {
            return Err("Mikrofon konnte nicht gestartet werden. Ist ein Mikrofon angeschlossen?".to_string());
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
) -> Result<serde_json::Value, String> {
    // Flag setzen -> Thread beendet sich selbst
    {
        let inner = state.lock().map_err(|e| e.to_string())?;
        let mut flag = inner.is_recording.lock().map_err(|e| e.to_string())?;
        *flag = false;
    }

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
    info!("Aufnahme gestoppt: {:.1}s, {} Samples @ {} Hz", duration_secs, samples.len(), sample_rate);

    // Auf 16kHz resamplen fuer kleinere Dateien (Whisper braucht sowieso 16kHz)
    let (save_samples, save_rate) = if sample_rate != 16000 {
        info!("Resample fuer Speicherung: {} Hz -> 16000 Hz", sample_rate);
        let resampled = resample_simple(&samples, sample_rate, 16000);
        (resampled, 16000u32)
    } else {
        (samples, sample_rate)
    };

    // Als WAV speichern
    let recordings_dir = std::path::Path::new("recordings");
    if !recordings_dir.exists() {
        std::fs::create_dir_all(recordings_dir)
            .map_err(|e| format!("Konnte recordings-Verzeichnis nicht erstellen: {}", e))?;
    }

    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let filename = format!("aufnahme_{}.wav", timestamp);
    let path = recordings_dir.join(&filename);

    write_wav(&save_samples, save_rate, &path)?;

    let file_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let path_str = path.to_string_lossy().to_string();
    info!("Aufnahme gespeichert: {} ({:.1}s, {} Bytes)", path_str, duration_secs, file_size);

    let _ = app.emit(
        "recording-stopped",
        serde_json::json!({
            "path": path_str,
            "duration_secs": duration_secs,
            "filename": filename,
            "file_size": file_size,
        }),
    );

    // Monitoring nach Stop automatisch wieder starten
    let (is_monitoring, gain_arc) = {
        let inner = state.lock().map_err(|e| e.to_string())?;
        (inner.is_monitoring.clone(), inner.gain.clone())
    };
    let app_handle = app.clone();
    is_monitoring.store(true, Ordering::Relaxed);

    std::thread::spawn(move || {
        // Kurz warten bis Aufnahme-Thread beendet ist
        std::thread::sleep(std::time::Duration::from_millis(500));

        let host = cpal::default_host();
        let device = match host.default_input_device() {
            Some(d) => d,
            None => return,
        };

        let supported_config = device
            .supported_input_configs()
            .ok()
            .and_then(|mut configs| configs.find(|c| c.min_sample_rate().0 <= 48000))
            .or_else(|| device.supported_input_configs().ok()?.next());

        let supported_config = match supported_config {
            Some(c) => c,
            None => return,
        };

        let config = supported_config.with_max_sample_rate();
        let channels = config.channels() as usize;

        let last_level: Arc<Mutex<f32>> = Arc::new(Mutex::new(0.0));
        let last_level_clone = last_level.clone();
        let gain_clone = gain_arc.clone();

        let err_fn = |err: cpal::StreamError| {
            error!("Monitoring-Stream-Fehler: {}", err);
        };

        let stream = match device.build_input_stream(
            &config.into(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let current_gain = *gain_clone.lock().unwrap();
                let mut mono = Vec::with_capacity(data.len() / channels);
                for chunk in data.chunks(channels) {
                    let avg: f32 = chunk.iter().sum::<f32>() / channels as f32;
                    mono.push(avg * current_gain);
                }
                let rms = if mono.is_empty() { 0.0 } else {
                    let sum_sq: f32 = mono.iter().map(|s| s * s).sum();
                    (sum_sq / mono.len() as f32).sqrt()
                };
                let db = if rms > 0.0 { 20.0 * rms.log10() } else { -60.0 };
                let level = (db.clamp(-60.0, 0.0) + 60.0) / 60.0;
                *last_level_clone.lock().unwrap() = level;
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
            let _ = app_handle.emit("audio-level", serde_json::json!({
                "level": level,
                "db": level * 60.0 - 60.0,
            }));
        }

        drop(stream);
        info!("Auto-Monitoring beendet");
    });

    Ok(serde_json::json!({
        "path": path_str,
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
    file.write_all(&(36 + data_size).to_le_bytes()).map_err(|e| e.to_string())?;
    file.write_all(b"WAVE").map_err(|e| e.to_string())?;
    file.write_all(b"fmt ").map_err(|e| e.to_string())?;
    file.write_all(&16u32.to_le_bytes()).map_err(|e| e.to_string())?;
    file.write_all(&1u16.to_le_bytes()).map_err(|e| e.to_string())?;
    file.write_all(&1u16.to_le_bytes()).map_err(|e| e.to_string())?;
    file.write_all(&sample_rate.to_le_bytes()).map_err(|e| e.to_string())?;
    let byte_rate = sample_rate * 2;
    file.write_all(&byte_rate.to_le_bytes()).map_err(|e| e.to_string())?;
    file.write_all(&2u16.to_le_bytes()).map_err(|e| e.to_string())?;
    file.write_all(&16u16.to_le_bytes()).map_err(|e| e.to_string())?;
    file.write_all(b"data").map_err(|e| e.to_string())?;
    file.write_all(&data_size.to_le_bytes()).map_err(|e| e.to_string())?;

    for &sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let pcm = if clamped < 0.0 {
            (clamped * 32768.0) as i16
        } else {
            (clamped * 32767.0) as i16
        };
        file.write_all(&pcm.to_le_bytes()).map_err(|e| e.to_string())?;
    }

    Ok(())
}
