use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};
use tracing::{error, info};

/// Globaler Aufnahme-State (cpal::Stream wird in einem Thread gehalten, nicht hier)
pub struct RecordingState {
    pub samples: Arc<Mutex<Vec<f32>>>,
    pub is_recording: Arc<Mutex<bool>>,
    pub sample_rate: Arc<Mutex<u32>>,
}

impl RecordingState {
    pub fn new() -> Self {
        Self {
            samples: Arc::new(Mutex::new(Vec::new())),
            is_recording: Arc::new(Mutex::new(false)),
            sample_rate: Arc::new(Mutex::new(48000)),
        }
    }
}

// Send+Sync manuell implementieren: wir speichern den Stream nicht hier
unsafe impl Send for RecordingState {}
unsafe impl Sync for RecordingState {}

/// Startet die Audio-Aufnahme vom Standard-Mikrofon
#[tauri::command]
pub async fn start_recording(
    app: AppHandle,
    state: tauri::State<'_, RecordingState>,
) -> Result<String, String> {
    // Pruefen ob schon aufgenommen wird
    {
        let is_rec = state.is_recording.lock().map_err(|e| e.to_string())?;
        if *is_rec {
            return Err("Aufnahme laeuft bereits".to_string());
        }
    }

    // Alte Samples loeschen
    {
        let mut samples = state.samples.lock().map_err(|e| e.to_string())?;
        samples.clear();
    }

    let samples = state.samples.clone();
    let is_recording = state.is_recording.clone();
    let sample_rate_arc = state.sample_rate.clone();

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
            .next()
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

        let err_fn = |err: cpal::StreamError| {
            error!("Audio-Stream-Fehler: {}", err);
        };

        let stream = device.build_input_stream(
            &config.into(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let should_record = *is_recording_clone.lock().unwrap();
                if !should_record {
                    return;
                }
                // Mono-Downmix
                let mut mono = Vec::with_capacity(data.len() / channels);
                for chunk in data.chunks(channels) {
                    let avg: f32 = chunk.iter().sum::<f32>() / channels as f32;
                    mono.push(avg);
                }
                let mut buf = samples_clone.lock().unwrap();
                buf.extend_from_slice(&mono);
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
        let is_rec = state.is_recording.lock().map_err(|e| e.to_string())?;
        if !*is_rec {
            return Err("Mikrofon konnte nicht gestartet werden. Ist ein Mikrofon angeschlossen?".to_string());
        }
    }

    let _ = app;

    Ok("Mikrofon aktiv".to_string())
}

/// Stoppt die Aufnahme und speichert das Ergebnis als WAV
#[tauri::command]
pub async fn stop_recording(
    app: AppHandle,
    state: tauri::State<'_, RecordingState>,
) -> Result<serde_json::Value, String> {
    // Flag setzen -> Thread beendet sich selbst
    {
        let mut flag = state.is_recording.lock().map_err(|e| e.to_string())?;
        *flag = false;
    }

    // Kurz warten bis Thread beendet
    std::thread::sleep(std::time::Duration::from_millis(300));

    let samples = {
        let s = state.samples.lock().map_err(|e| e.to_string())?;
        s.clone()
    };

    let sample_rate = {
        let sr = state.sample_rate.lock().map_err(|e| e.to_string())?;
        *sr
    };

    if samples.is_empty() {
        return Err("Aufnahme enthaelt keine Samples. Mikrofon moeglicherweise nicht verbunden.".to_string());
    }

    let duration_secs = samples.len() as f64 / sample_rate as f64;
    info!("Aufnahme gestoppt: {:.1}s, {} Samples @ {} Hz", duration_secs, samples.len(), sample_rate);

    // Als WAV speichern
    let recordings_dir = std::path::Path::new("recordings");
    if !recordings_dir.exists() {
        std::fs::create_dir_all(recordings_dir)
            .map_err(|e| format!("Konnte recordings-Verzeichnis nicht erstellen: {}", e))?;
    }

    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let filename = format!("aufnahme_{}.wav", timestamp);
    let path = recordings_dir.join(&filename);

    write_wav(&samples, sample_rate, &path)?;

    let path_str = path.to_string_lossy().to_string();
    info!("Aufnahme gespeichert: {} ({:.1}s)", path_str, duration_secs);

    let _ = app.emit(
        "recording-stopped",
        serde_json::json!({
            "path": path_str,
            "duration_secs": duration_secs,
            "filename": filename,
        }),
    );

    Ok(serde_json::json!({
        "path": path_str,
        "duration_secs": duration_secs,
        "filename": filename,
        "sample_count": samples.len(),
    }))
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
