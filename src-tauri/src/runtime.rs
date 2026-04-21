use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager, State};
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuntimeProfile {
    pub config_version: u32,
    pub detected_at: String,
    pub os: String,
    pub arch: String,
    pub first_run_completed: bool,
    pub system: SystemHardware,
    pub whisper: BackendStatus,
    pub analysis: BackendStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SystemHardware {
    pub gpu_present: bool,
    pub gpu_vendor: Option<String>,
    pub gpu_model: Option<String>,
    pub gpu_backend: String,
    pub detection_notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BackendStatus {
    pub mode: String,
    pub effective_backend: String,
    pub reason: String,
}

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| {
        format!(
            "App-Konfigurationspfad konnte nicht ermittelt werden: {}",
            e
        )
    })?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Konfigurationsordner konnte nicht erstellt werden: {}", e))?;
    Ok(dir.join("runtime_profile.json"))
}

fn save_profile(app: &AppHandle, profile: &RuntimeProfile) -> Result<(), String> {
    let path = config_path(app)?;
    let content = serde_json::to_string_pretty(profile)
        .map_err(|e| format!("Runtime-Profil konnte nicht serialisiert werden: {}", e))?;
    std::fs::write(&path, content)
        .map_err(|e| format!("Runtime-Profil konnte nicht gespeichert werden: {}", e))
}

fn load_profile(app: &AppHandle) -> Result<Option<RuntimeProfile>, String> {
    let path = config_path(app)?;
    if !path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("Runtime-Profil konnte nicht gelesen werden: {}", e))?;
    let profile = serde_json::from_str::<RuntimeProfile>(&content)
        .map_err(|e| format!("Runtime-Profil ist ungueltig: {}", e))?;
    Ok(Some(profile))
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        None
    } else {
        Some(stdout)
    }
}

fn detect_system_hardware() -> SystemHardware {
    let mut hardware = SystemHardware {
        gpu_backend: "cpu".to_string(),
        ..Default::default()
    };

    if let Some(output) =
        command_output("nvidia-smi", &["--query-gpu=name", "--format=csv,noheader"])
    {
        let first_gpu = output.lines().next().unwrap_or_default().trim().to_string();
        hardware.gpu_present = true;
        hardware.gpu_vendor = Some("NVIDIA".to_string());
        hardware.gpu_model = Some(first_gpu);
        hardware.gpu_backend = "cuda".to_string();
        hardware
            .detection_notes
            .push("NVIDIA-GPU ueber nvidia-smi erkannt".to_string());
        return hardware;
    }

    if let Some(output) = command_output("system_profiler", &["SPDisplaysDataType"]) {
        let lower = output.to_ascii_lowercase();
        if lower.contains("metal") || lower.contains("chipset model") {
            let model = output
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .map(|(_, value)| value.trim().to_string())
                })
                .filter(|value| !value.is_empty());
            hardware.gpu_present = true;
            hardware.gpu_vendor = Some("Apple".to_string());
            hardware.gpu_model = model;
            hardware.gpu_backend = "metal".to_string();
            hardware
                .detection_notes
                .push("Apple-GPU ueber system_profiler erkannt".to_string());
            return hardware;
        }
    }

    if let Some(output) = command_output("rocm-smi", &["--showproductname"]) {
        let model = output
            .lines()
            .find(|line| line.to_ascii_lowercase().contains("card series"))
            .map(|line| line.trim().to_string())
            .or_else(|| output.lines().next().map(|line| line.trim().to_string()));
        hardware.gpu_present = true;
        hardware.gpu_vendor = Some("AMD".to_string());
        hardware.gpu_model = model;
        hardware.gpu_backend = "rocm".to_string();
        hardware
            .detection_notes
            .push("AMD-GPU ueber rocm-smi erkannt".to_string());
        return hardware;
    }

    if let Some(output) = command_output("lspci", &[]) {
        for line in output.lines() {
            let lower = line.to_ascii_lowercase();
            if lower.contains("vga compatible controller") || lower.contains("3d controller") {
                if lower.contains("nvidia") {
                    hardware.gpu_present = true;
                    hardware.gpu_vendor = Some("NVIDIA".to_string());
                    hardware.gpu_model = Some(line.trim().to_string());
                    hardware.gpu_backend = "cuda".to_string();
                    hardware
                        .detection_notes
                        .push("NVIDIA-GPU ueber lspci erkannt".to_string());
                    return hardware;
                }
                if lower.contains("amd") || lower.contains("advanced micro devices") {
                    hardware.gpu_present = true;
                    hardware.gpu_vendor = Some("AMD".to_string());
                    hardware.gpu_model = Some(line.trim().to_string());
                    hardware.gpu_backend = "rocm".to_string();
                    hardware
                        .detection_notes
                        .push("AMD-GPU ueber lspci erkannt".to_string());
                    return hardware;
                }
                if lower.contains("intel") {
                    hardware.gpu_present = true;
                    hardware.gpu_vendor = Some("Intel".to_string());
                    hardware.gpu_model = Some(line.trim().to_string());
                    hardware.gpu_backend = "integrated".to_string();
                    hardware
                        .detection_notes
                        .push("Intel-GPU erkannt, aktuell ohne Whisper-GPU-Profil".to_string());
                    return hardware;
                }
            }
        }
    }

    hardware
        .detection_notes
        .push("Keine dedizierte GPU sicher erkannt, CPU-Profil aktiv".to_string());
    hardware
}

fn detect_profile() -> RuntimeProfile {
    let hardware = detect_system_hardware();
    let detected_at = chrono::Local::now().to_rfc3339();

    let whisper = match hardware.gpu_backend.as_str() {
        "cuda" | "metal" | "rocm" => BackendStatus {
            mode: "auto".to_string(),
            effective_backend: "cpu".to_string(),
            reason: "GPU erkannt, aber Whisper laeuft aktuell noch im stabilen CPU-Profil. GPU-Build kann spaeter gezielt aktiviert werden.".to_string(),
        },
        _ => BackendStatus {
            mode: "auto".to_string(),
            effective_backend: "cpu".to_string(),
            reason: "Kein kompatibles Whisper-GPU-Profil aktiv; CPU wird verwendet.".to_string(),
        },
    };

    let analysis = if hardware.gpu_present {
        BackendStatus {
            mode: "auto".to_string(),
            effective_backend: "managed-by-ollama".to_string(),
            reason: "GPU erkannt. Ob die Analyse GPU nutzt, entscheidet Ollama zur Laufzeit auf diesem System.".to_string(),
        }
    } else {
        BackendStatus {
            mode: "auto".to_string(),
            effective_backend: "cpu".to_string(),
            reason: "Keine GPU erkannt; Ollama wird voraussichtlich CPU verwenden.".to_string(),
        }
    };

    RuntimeProfile {
        config_version: 1,
        detected_at,
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        first_run_completed: true,
        system: hardware,
        whisper,
        analysis,
    }
}

pub fn initialize_runtime_profile(app: &AppHandle) -> RuntimeProfile {
    match load_profile(app) {
        Ok(Some(profile)) => {
            info!("Runtime-Profil aus Konfiguration geladen");
            profile
        }
        Ok(None) => {
            info!("Kein Runtime-Profil gefunden, fuehre Initial-Check aus");
            let profile = detect_profile();
            if let Err(err) = save_profile(app, &profile) {
                warn!("Runtime-Profil konnte nicht gespeichert werden: {}", err);
            }
            profile
        }
        Err(err) => {
            warn!("Runtime-Profil konnte nicht geladen werden: {}", err);
            detect_profile()
        }
    }
}

#[tauri::command]
pub async fn get_runtime_profile(
    app: AppHandle,
    state: State<'_, Arc<Mutex<RuntimeProfile>>>,
) -> Result<RuntimeProfile, String> {
    let cached = state.lock().map_err(|e| e.to_string())?.clone();
    if cached.first_run_completed {
        Ok(cached)
    } else {
        let profile = initialize_runtime_profile(&app);
        *state.lock().map_err(|e| e.to_string())? = profile.clone();
        Ok(profile)
    }
}

#[tauri::command]
pub async fn refresh_runtime_profile(
    app: AppHandle,
    state: State<'_, Arc<Mutex<RuntimeProfile>>>,
) -> Result<RuntimeProfile, String> {
    let profile = detect_profile();
    save_profile(&app, &profile)?;
    *state.lock().map_err(|e| e.to_string())? = profile.clone();
    Ok(profile)
}
