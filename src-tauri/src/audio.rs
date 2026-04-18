use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::Path;
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use tracing::{debug, info, warn};

/// Laenge eines Chunks in Sekunden
const CHUNK_DURATION_SECS: f64 = 30.0;
/// Ueberlappung zwischen Chunks in Sekunden
const CHUNK_OVERLAP_SECS: f64 = 1.5;
/// Ziel-Samplerate (Whisper-Anforderung)
const TARGET_SAMPLE_RATE: u32 = 16000;
/// Maximale Audiodauer in Sekunden (4 Stunden)
const MAX_AUDIO_DURATION_SECS: f64 = 4.0 * 3600.0;
/// Unterstuetzte Dateiendungen
const SUPPORTED_EXTENSIONS: &[&str] = &["wav", "mp3", "flac", "ogg", "m4a", "aac", "wma", "opus"];

/// Metadaten eines Audio-Chunks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioChunk {
    /// Index des Chunks (0-basiert)
    pub index: usize,
    /// Startzeit in Sekunden
    pub start_secs: f64,
    /// Endzeit in Sekunden
    pub end_secs: f64,
    /// PCM-Samples (16-bit, mono, 16 kHz)
    pub samples: Vec<f32>,
}

/// Ergebnis der Audio-Analyse
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioInfo {
    /// Gesamtdauer in Sekunden
    pub duration_secs: f64,
    /// Samplerate der Quelldatei
    pub sample_rate: u32,
    /// Anzahl der Kanäle
    pub channels: usize,
    /// Anzahl der zu erwartenden Chunks
    pub total_chunks: usize,
    /// Dateiname
    pub filename: String,
    /// Dateigroesse in Bytes
    pub file_size: u64,
}

/// Liest eine Audiodatei und dekodiert sie komplett zu 16kHz Mono PCM
pub fn load_audio(path: &Path) -> Result<(AudioInfo, Vec<f32>), String> {
    // Datei-Validierung
    if !path.exists() {
        return Err(format!("Datei nicht gefunden: {}", path.display()));
    }

    // Dateiendung pruefen
    if let Some(ext) = path.extension() {
        let ext_lower = ext.to_string_lossy().to_lowercase();
        if !SUPPORTED_EXTENSIONS.contains(&ext_lower.as_str()) {
            warn!(
                "Möglicherweise nicht unterstütztes Format: .{} (unterstützt: {})",
                ext_lower,
                SUPPORTED_EXTENSIONS.join(", ")
            );
        }
    }

    let filename = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let file_size = path.metadata().map(|m| m.len()).unwrap_or(0);

    // Dateigroesse begrenzen (2 GB)
    if file_size > 2 * 1024 * 1024 * 1024 {
        return Err(format!(
            "Datei zu gross: {:.0} MB. Maximum: 2048 MB.",
            file_size as f64 / (1024.0 * 1024.0)
        ));
    }

    let file = File::open(path).map_err(|e| format!("Datei konnte nicht geoeffnet werden: {}", e))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension() {
        hint.with_extension(ext.to_string_lossy().as_ref());
    }

    let format_opts = FormatOptions {
        enable_gapless: true,
        ..Default::default()
    };
    let metadata_opts = MetadataOptions::default();
    let decoder_opts = DecoderOptions::default();

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &format_opts, &metadata_opts)
        .map_err(|e| format!("Audio-Format nicht erkannt: {}", e))?;

    let mut format_reader = probed.format;
    let track = format_reader
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("Kein Audio-Track gefunden")?;

    let track_id = track.id;
    let codec_params = &track.codec_params;
    let orig_sample_rate = codec_params.sample_rate.unwrap_or(44100);
    let _channels = codec_params.channels.map(|c| c.count()).unwrap_or(1);

    let mut decoder = symphonia::default::get_codecs()
        .make(codec_params, &decoder_opts)
        .map_err(|e| format!("Decoder konnte nicht erstellt werden: {}", e))?;

    // Alle Samples dekodieren
    let mut all_samples: Vec<f32> = Vec::new();

    loop {
        match format_reader.next_packet() {
            Ok(packet) => {
                if packet.track_id() != track_id {
                    continue;
                }
                match decoder.decode(&packet) {
                    Ok(audio_buf) => {
                        // Mono-Downmix: alle Kanaele mitteln
                        let spec = *audio_buf.spec();
                        let num_frames = audio_buf.frames();
                        let num_channels = spec.channels.count();

                        for frame in 0..num_frames {
                            let mut mono_sample = 0.0f32;
                            for ch in 0..num_channels {
                                mono_sample += match audio_buf {
                                    AudioBufferRef::U8(ref buf) => buf.chan(ch)[frame] as f32 / u8::MAX as f32,
                                    AudioBufferRef::S16(ref buf) => buf.chan(ch)[frame] as f32 / i16::MAX as f32,
                                    AudioBufferRef::S24(ref buf) => buf.chan(ch)[frame].0 as f32 / (1i32 << 23) as f32,
                                    AudioBufferRef::S32(ref buf) => buf.chan(ch)[frame] as f32 / i32::MAX as f32,
                                    AudioBufferRef::F32(ref buf) => buf.chan(ch)[frame],
                                    AudioBufferRef::F64(ref buf) => buf.chan(ch)[frame] as f32,
                                    _ => 0.0, // Unbekannte Formate werden als 0.0 behandelt
                                };
                            }
                            all_samples.push(mono_sample / num_channels as f32);
                        }
                    }
                    Err(SymphoniaError::DecodeError(_)) => continue,
                    Err(e) => {
                        warn!("Dekodierungsfehler: {}", e);
                        break;
                    }
                }
            }
            Err(SymphoniaError::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(_) => break,
        }
    }

    let total_duration = all_samples.len() as f64 / orig_sample_rate as f64;

    // Dauer pruefen
    if total_duration > MAX_AUDIO_DURATION_SECS {
        return Err(format!(
            "Audio zu lang: {:.0} Minuten. Maximum: {:.0} Minuten.",
            total_duration / 60.0,
            MAX_AUDIO_DURATION_SECS / 60.0
        ));
    }

    if total_duration < 0.1 {
        return Err("Audio ist kuerzer als 0.1 Sekunden. Nichts zu transkribieren.".to_string());
    }

    // Resampling auf 16 kHz falls noetig
    let samples_16k = if orig_sample_rate != TARGET_SAMPLE_RATE {
        info!(
            "Resampling: {} Hz -> {} Hz ({} Samples)",
            orig_sample_rate,
            TARGET_SAMPLE_RATE,
            all_samples.len()
        );
        resample(&all_samples, orig_sample_rate, TARGET_SAMPLE_RATE)?
    } else {
        all_samples
    };

    let total_chunks = compute_chunk_count(total_duration, CHUNK_DURATION_SECS, CHUNK_OVERLAP_SECS);

    let info = AudioInfo {
        duration_secs: total_duration,
        sample_rate: TARGET_SAMPLE_RATE,
        channels: 1,
        total_chunks,
        filename,
        file_size,
    };

    info!(
        "Audio geladen: {} ({:.1}s, {} Hz, {} Kanäle, {} Chunks erwartet)",
        info.filename,
        info.duration_secs,
        info.sample_rate,
        info.channels,
        info.total_chunks
    );

    Ok((info, samples_16k))
}

/// Teilt Samples in 30-Sekunden-Chunks mit Ueberlappung
pub fn chunk_audio(samples: &[f32], sample_rate: u32) -> Vec<AudioChunk> {
    let chunk_size = (CHUNK_DURATION_SECS * sample_rate as f64) as usize;
    let overlap_size = (CHUNK_OVERLAP_SECS * sample_rate as f64) as usize;
    let step = chunk_size - overlap_size;
    let total_duration = samples.len() as f64 / sample_rate as f64;

    let mut chunks = Vec::new();
    let mut offset = 0usize;
    let mut index = 0;

    while offset < samples.len() {
        let end = std::cmp::min(offset + chunk_size, samples.len());
        let chunk_samples = samples[offset..end].to_vec();

        let start_secs = offset as f64 / sample_rate as f64;
        let end_secs = end as f64 / sample_rate as f64;

        debug!(
            "Chunk {}: {:.1}s - {:.1}s ({} Samples)",
            index,
            start_secs,
            end_secs,
            chunk_samples.len()
        );

        chunks.push(AudioChunk {
            index,
            start_secs,
            end_secs,
            samples: chunk_samples,
        });

        offset += step;
        index += 1;

        // Letzter Chunk: wenn weniger als 1 Sekunde uebrig ist, nicht noch einen Chunk erstellen
        if samples.len().saturating_sub(offset) < (sample_rate as usize) {
            break;
        }
    }

    info!(
        "Chunking abgeschlossen: {} Chunks aus {:.1}s Audio ({}s Fenster, {}s Ueberlappung)",
        chunks.len(),
        total_duration,
        CHUNK_DURATION_SECS,
        CHUNK_OVERLAP_SECS
    );

    chunks
}

/// Berechnet die erwartete Anzahl Chunks
fn compute_chunk_count(duration: f64, chunk_dur: f64, overlap: f64) -> usize {
    if duration <= chunk_dur {
        return 1;
    }
    let step = chunk_dur - overlap;
    ((duration - chunk_dur) / step + 1.0).ceil() as usize
}

/// Resampling mit linearer Interpolation (einfacher Ansatz)
fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Result<Vec<f32>, String> {
    if from_rate == to_rate {
        return Ok(samples.to_vec());
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

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_chunk_count() {
        // 30s Audio = 1 Chunk
        assert_eq!(compute_chunk_count(30.0, 30.0, 1.5), 1);
        // 60s Audio: Chunk 0 (0-30s), Chunk 1 (28.5-58.5s), Chunk 2 (57-60s)
        assert_eq!(compute_chunk_count(60.0, 30.0, 1.5), 3);
        // 90s Audio: 4 Chunks
        assert_eq!(compute_chunk_count(90.0, 30.0, 1.5), 4);
        // 5s Audio = 1 Chunk
        assert_eq!(compute_chunk_count(5.0, 30.0, 1.5), 1);
    }

    #[test]
    fn test_chunk_audio() {
        // 60 Sekunden Audio bei 16kHz = 960000 Samples
        // Mit 30s Fenstern und 1.5s Ueberlappung (Schritt = 28.5s):
        // Chunk 0: 0-30s, Chunk 1: 28.5-58.5s, Chunk 2: 57-60s
        let samples = vec![0.5f32; 960000];
        let chunks = chunk_audio(&samples, 16000);
        assert!(chunks.len() >= 2);
        assert_eq!(chunks[0].index, 0);
        // Chunk 0: 0s - 30s
        assert!((chunks[0].start_secs - 0.0).abs() < 0.1);
        assert!((chunks[0].end_secs - 30.0).abs() < 0.1);
        // Chunk 1: startet bei 28.5s
        assert!((chunks[1].start_secs - 28.5).abs() < 0.1);
    }

    #[test]
    fn test_resample() {
        let samples = vec![1.0f32, 2.0, 3.0, 4.0];
        let resampled = resample(&samples, 16000, 32000).unwrap();
        // Verdopplung der Samplerate -> ca. doppelte Sample-Anzahl
        assert_eq!(resampled.len(), 8);
    }
}
