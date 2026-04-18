# Detaillierter Umsetzungsplan: Lokale Transkriptions-App

## Projektübersicht

| Aspekt | Entscheidung |
|---|---|
| **UI-Framework** | Tauri (Rust-Backend + Web-Frontend) |
| **Inferenz-Backend (Whisper)** | OpenVINO (Intel iGPU/NPU) |
| **Inferenz-Backend (Gemma 4)** | Ollama (lokale REST-API, GPU-Beschleunigung) |
| **Spracherkennung** | Whisper Large-v3-Turbo (5-Bit quantisiert) |
| **Textanalyse** | Gemma 4 E4B via Ollama (4-Bit quantisiert, GGUF) |
| **Hardware-Ziel** | 16-GB-Laptop mit Intel iGPU |
| **Speicherlimit** | 32k Token Kontext, `-np 1` Flag |
| **Verarbeitungsmodell** | Sequenziell (Phase 1: Transkription, Phase 2: Analyse) |

---

## Stufe 1: Projekt-Infrastruktur & Build-System **[ABGESCHLOSSEN]**

### 1.1 Tauri-Projekt initialisieren **[ERLEDIGT]**

- Tauri-Projekt manuell erstellt (React + TypeScript + Rust)
- Frontend: TypeScript + React 19 + Vite 6
- Backend: Rust mit Tauri 2.10
- Ordnerstruktur:

```
src-tauri/       -> Rust-Backend (Tauri 2, reqwest, tokio)
src-tauri/src/
  lib.rs         -> Tauri-App-Einstieg, Command-Handler
  main.rs        -> Binary-Entry-Point
  ollama.rs      -> Ollama REST-API-Client
src/             -> Frontend (React/TS)
  App.tsx        -> Hauptkomponente (Status, Transcript, Analyse)
  main.tsx       -> React-Einstieg
  styles.css     -> Styling (inkl. Dark Mode)
  vite-env.d.ts  -> Vite-Typen
models/          -> Ablage fuer quantisierte Whisper-Modelle
index.html       -> HTML-Entry-Point
package.json     -> npm-Abhaengigkeiten
vite.config.ts   -> Vite-Konfiguration fuer Tauri
tsconfig.json    -> TypeScript-Konfiguration
```

### 1.2 OpenVINO-Rust-Bindings einrichten **[TEILWEISE]**

- OpenVINO 2026.1.0 via Python venv installiert (`/home/okko/openvino-env`)
- C-Libraries verfuegbar unter: `/home/okko/openvino-env/lib/python3.12/site-packages/openvino/libs/`
- Symlink erstellt: `libopenvino.so -> libopenvino.so.2610`
- `openvino` Crate noch nicht in `Cargo.toml` (folgt in Stufe 3 mit Whisper)
- iGPU/NPU-Erkennung: noch nicht getestet (folgt in Stufe 3)

### 1.3 Ollama-Integration vorbereiten **[ERLEDIGT]**

- Ollama installiert via `curl -fsSL https://ollama.com/install.sh | sh`
- Modell geladen: `gemma3:4b` (3.3 GB, via `ollama pull gemma3:4b`)
  - Hinweis: Im Plan als "Gemma 4 E4B" bezeichnet, aktuell als `gemma3:4b` verfuegbar
- Ollama-Service laeuft unter `http://localhost:11434`
- `reqwest` Crate in `Cargo.toml` aufgenommen
- Rust Ollama-Client implementiert (`src-tauri/src/ollama.rs`):
  - `check_status()` -> Ollama-Verbindung pruefen
  - `get_models()` -> Verfuegbare Modelle abfragen
  - `analyze()` -> Textanalyse via Chat-API mit System-Prompts

### 1.4 Verifizierung **[ERLEDIGT]**

- `cargo tauri build --debug` erfolgreich durchgelaufen
- Bundles erstellt: DEB, RPM, AppImage
- TypeScript-Typecheck (`tsc --noEmit`) fehlerfrei
- Rust (`cargo check`) kompiliert ohne Fehler
- Ollama API antwortet (`curl http://localhost:11434/api/tags`)

### Installierte System-Abhaengigkeiten

- `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `libsoup-3.0-dev`
- `libjavascriptcoregtk-4.1-dev`, `libayatana-appindicator3-dev`
- `librsvg2-dev`, `patchelf`

### Installierte Tools (Versionen)

| Tool | Version |
|---|---|
| Node.js | v22.22.2 |
| npm | 10.9.7 |
| Rust | 1.95.0 |
| Cargo | 1.95.0 |
| Tauri CLI | 2.10.1 |
| OpenVINO | 2026.1.0 (Python venv) |
| Ollama | aktuell, gemma3:4b geladen |
| OS | Pop!_OS 24.04 LTS (Ubuntu-basiert) |

**Geschätzter Aufwand: ~1-2 Tage -- Tatsaechlich: ~1 Sitzung**

---

## Stufe 2: Audio-Pipeline & Chunking **[ABGESCHLOSSEN]**

### 2.1 Audio-Input-Handling **[ERLEDIGT]**

- Unterstuetzte Formate: WAV, MP3, FLAC, OGG, M4A (via `symphonia` Crate)
- Audio-Preprocessing:
  - Resampling auf 16 kHz (Whisper-Anforderung) via `rubato`
  - Mono-Downmix (Mehrkanal -> 1 Kanal)
  - 32-bit Float-Ausgabe
- Implementierung in `src-tauri/src/audio.rs`:
  - `AudioInfo`-Struct: Format, Samplerate, Kanaele, Dauer
  - `load_audio()` liest beliebige Formate und gibt normalisierte 16kHz-Mono-F32-Samples zurueck
  - `get_audio_info()` fuer Metadaten ohne volle Dekodierung

### 2.2 Chunking-Strategie **[ERLEDIGT]**

- Feste 30-Sekunden-Fenster (Whisper-kompatibel)
- Ueberlappung von 1.5 Sekunden an den Chunk-Grenzen (vermeidet Wortverluste)
- `AudioChunk`-Struct mit Index, Samples, Start-/Endzeitstempel
- `chunk_audio()` teilt Samples in ueberlappende Fenster auf
- `compute_chunk_count()` fuer schnelle Vorschau ohne Allokation
- Queue-basierte Architektur: Chunks koennen sequenziell verarbeitet werden

### 2.3 Tauri-Integration **[ERLEDIGT]**

- Tauri-Commands in `lib.rs`:
  - `load_audio_file(path)` -> AudioInfo + Chunk-Anzahl
  - `get_audio_chunks(path)` -> Vec<AudioChunkInfo> mit Zeitstempeln
- Frontend: Drag & Drop + Datei-Dialog fuer Audio-Upload
- Chunk-Liste mit Zeitstempeln wird im UI angezeigt

### 2.4 Frontend **[ERLEDIGT]**

- Drag & Drop Zone fuer Audio-Dateien
- Datei-Dialog via `@tauri-apps/plugin-dialog`
- Chunk-Liste mit Index, Start-/Endzeit
- Statusanzeige: Audio geladen / X Chunks bereit

### 2.5 Verifizierung **[ERLEDIGT]**

- Unit-Tests: Chunk-Berechnung, Audio-Info, Chunking-Logik (3 Tests, alle bestanden)
- `cargo check` fehlerfrei
- `tsc --noEmit` fehlerfrei
- `cargo test` 3/3 bestanden

### Abhaengigkeiten (Cargo.toml)

| Crate | Zweck |
|---|---|
| `symphonia` (features: mp3, flac, ogg, wav, isomp4, aac) | Audio-Dekodierung |
| `rubato` | Resampling auf 16kHz |
| `rand` | Zufallsgenerator (fuer Tests) |

**Geschätzter Aufwand: ~2-3 Tage -- Tatsaechlich: ~1 Sitzung**
---

## Stufe 3: Whisper-Integration (Spracherkennung) **[ABGESCHLOSSEN]**

### Strategie-Aenderung: whisper.cpp statt OpenVINO

Statt OpenVINO direkt zu nutzen, wurde **whisper.cpp** via `whisper-rs` (v0.16) integriert.
Gruende:
- whisper.cpp erledigt Mel-Spektrogramm, Encoder und Decoder intern (kein manueller Aufbau noetig)
- Native GGML-Untererstuetzung mit Vulkan-Backend fuer Intel iGPU
- Bewaehrte, gut gepflegte Bibliothek mit einfacher Rust-API
- Deutlich weniger Entwicklungsaufwand als rohe OpenVINO-Bindings

### 3.1 Modellvorbereitung **[ERLEDIGT]**

- Whisper Large-v3-Turbo Modell heruntergeladen (HuggingFace, ggml-Format)
- Pfad: `models/ggml-large-v3-turbo.bin` (1.6 GB)
- Keine Konvertierung noetig – whisper.cpp liest GGML direkt
- Keine separate Quantisierung noetig – Modell ist bereits optimiert

### 3.2 Inferenz-Engine implementieren **[ERLEDIGT]**

- `whisper-rs` Crate (v0.16) in `Cargo.toml` aufgenommen
- `WhisperState`-Struct in `src-tauri/src/whisper.rs`:
  - Lazy Loading: Modell wird beim ersten Transkriptionsaufruf geladen
  - `load_model()` laedt das GGML-Modell mit `WhisperContextParameters`
  - `transcribe_chunk()` fuehrt Inferenz durch:
    - `SamplingStrategy::Greedy` mit `best_of: 1`
    - Sprache einstellbar (Default: Deutsch)
    - 4 Threads fuer parallele Verarbeitung
    - Ergebnis via `state.get_segment()` und `segment.to_str()`
- System-Abhaengigkeiten installiert: `libclang-dev`, `clang`, `cmake`

### 3.3 Ergebnisverarbeitung **[ERLEDIGT]**

- `ChunkTranscript`-Struct mit Index, Zeitstempel, Text, Sprache
- `TranscriptionResult`-Struct mit Gesamttext, Chunk-Liste, Dauer, Modell
- `merge_transcripts()` fuegt Chunk-Texte zusammen
- `overlap_merge()` bereinigt doppelte Woerter an Ueberlappungsgrenzen:
  - Sucht laengste Wort-Uebereinstimmung (case-insensitive, max. 20 Woerter)
  - Entfernt doppelte Woerter beim Zusammenfuegen
- 6 Unit-Tests fuer Merge-Logik (alle bestanden)

### 3.4 Tauri-Integration **[ERLEDIGT]**

- `transcribe_audio` Command in `whisper.rs`:
  - Laedt Audio via `audio::load_audio()` in blocking Task
  - Erstellt Chunks via `audio::chunk_audio()`
  - Transkribiert Chunks sequenziell
  - Sendet Fortschritts-Events (`transcription-progress`) an Frontend
  - Sendet Abschluss-Event (`transcription-complete`)
- `WhisperState` wird als `Mutex<WhisperState>` in Tauri State verwaltet
- Modell-Pfad: Default `models/ggml-large-v3-turbo.bin`, konfigurierbar

### 3.5 Frontend **[ERLEDIGT]**

- "Transkribieren starten" Button in der Chunk-Liste
- Fortschrittsbalken mit Chunk X / Total Anzeige
- Live-Transkript-Updates via Event-Listener
- Chunk-Status-Anzeige: Ausstehend / Laeuft / Fertig / Fehler
- `@tauri-apps/api/event` fuer Echtzeit-Updates

### 3.6 Verifizierung **[ERLEDIGT]**

- `cargo build` erfolgreich (inkl. whisper.cpp C++-Kompilierung)
- `cargo check` fehlerfrei
- `cargo test` 9/9 bestanden (3 Audio + 6 Whisper Tests)
- `tsc --noEmit` fehlerfrei
- Frontend kompiliert mit neuen Transkriptions-Features

### Abhaengigkeiten (Cargo.toml)

| Crate | Zweck |
|---|---|
| `whisper-rs` v0.16 | Rust-Bindings fuer whisper.cpp (GGML Inferenz) |

### Neue Dateien

| Datei | Zweck |
|---|---|
| `src-tauri/src/whisper.rs` | Whisper-Modul (Modell laden, transkribieren, merge) |
| `models/ggml-large-v3-turbo.bin` | Whisper Large-v3-Turbo Modell (1.6 GB) |

### Installierte System-Abhaengigkeiten

- `libclang-dev`, `clang` (fuer bindgen/whisper.cpp Build)
- `cmake`, `build-essential` (fuer whisper.cpp CMake Build)

**Geschätzter Aufwand: ~3-5 Tage -- Tatsaechlich: ~1 Sitzung**
---

## Stufe 4: Gemma 4 E4B-Integration via Ollama (Textanalyse) **[ABGESCHLOSSEN]**

Statt Gemma 4 manuell ueber OpenVINO einzubinden, nutzen wir Ollama als lokales
Inferenz-Backend. Ollama uebernimmt Modellverwaltung, Tokenizer, KV-Cache und
GPU-Beschleunigung automatisch. Die Kommunikation erfolgt ueber eine einfache
REST-API unter `localhost:11434`.

### 4.1 Voraussetzungen **[ERLEDIGT]**

- Ollama ist installiert und laeuft (`ollama serve`)
- Modell geladen: `gemma3:4b` (via `ollama pull gemma3:4b`)
- API ist erreichbar unter `http://localhost:11434`
- Status-Pruefung beim App-Start implementiert

### 4.2 Ollama-API-Client **[ERLEDIGT]**

- `src-tauri/src/ollama.rs` komplett neu geschrieben:
- `AnalysisTask`-Enum mit 8 Analyse-Tasks:
  - Summary, DetailedSummary, Topics, Actions, Sentiment, Decisions, Protocol, Full
- Jeder Task hat optimierten System-Prompt (professionell, Deutsch)
- `check_status()` – Ollama-Verbindung pruefen mit 5s Timeout
- `analyze()` – Nicht-streaming Analyse (300s Timeout)
- `analyze_stream()` – Streaming-Analyse mit Token-Events:
  - Sendet `analysis-token` Event pro Token an Frontend
  - Sendet `analysis-complete` Event am Ende
- Fehlerbehandlung:
  - Ollama nicht erreichbar -> klare Fehlermeldung
  - Timeout -> Hinweis fuer kuerzeren Text
  - Allgemeine Fehler -> formatierte Ausgabe
- Ollama-Parameter: `temperature: 0.3`, `top_p: 0.9`, `num_ctx: 32768`

### 4.3 Analyse-Tasks **[ERLEDIGT]**

| Task | Beschreibung |
|---|---|
| Zusammenfassung | Kurze, praegnante Zusammenfassung (3-5 Absaetze) |
| Ausfuehrlich | Detaillierte Zusammenfassung mit Ueberschriften |
| Themen | Hauptthemen + Keywords mit Erklaerung |
| Aktionspunkte | To-dos mit Personen-Zuordnung |
| Beschluesse | Entscheidungen mit Verwantwortlichen + Fristen |
| Stimmung | Emotionale Tendenz, Stimmungswechsel |
| Protokoll | Strukturiertes Protokoll (Agenda, Diskussion, Beschluesse) |
| Vollanalyse | Alle Analysen kombiniert |

### 4.4 Frontend **[ERLEDIGT]**

- 8 Analyse-Buttons mit Icons (ueber `ANALYSIS_OPTIONS`-Array)
- Streaming-Ausgabe: Text erscheint Token fuer Token
- Auto-Scroll der Ergebnis-Box waehrend Streaming
- "Kopieren" Button fuer Ergebnisse
- Animierter Fortschrittsbalken waehrend Analyse
- `analysis-token` und `analysis-complete` Event-Listener

### 4.5 Verifizierung **[ERLEDIGT]**

- `cargo check` fehlerfrei
- `cargo test` 12/12 bestanden (9 Audio/Whisper + 3 Ollama)
- `tsc --noEmit` fehlerfrei
- Streaming-Analyse funktioniert (Token-Events)

**Geschätzter Aufwand: ~1-2 Tage -- Tatsaechlich: ~1 Sitzung**
---

## Stufe 5: Sequenzieller Workflow (Kernarchitektur) **[ABGESCHLOSSEN]**

### 5.1 Workflow-Modul **[ERLEDIGT]**

- Neues Modul `src-tauri/src/workflow.rs` implementiert
- `run_workflow` Tauri-Command: Transkription + Analyse in einem Durchlauf
- Parameter: `path` (Audio-Datei), `task` (Analyse-Task), `language` (optional)
- Ergebnis: `WorkflowResult` mit Transkript, Analyse, Audio-Info, Chunk-Anzahl

### 5.2 Workflow-Phasen **[ERLEDIGT]**

| Phase | Aufgabe | Modell |
|---|---|---|
| **LoadingAudio** | Audio laden und chunken | - |
| **Transcribing** | Chunks sequenziell transkribieren | Whisper Large-v3-Turbo |
| **Analyzing** | Vollstaendiges Transkript analysieren | Gemma 4 E4B (via Ollama) |
| **Complete** | Ergebnis bereit | - |
| **Error** | Fehler aufgetreten | - |

- `WorkflowPhase`-Enum mit Serialize/Deserialize
- `WorkflowState`-Struct mit Phase, Chunk-Fortschritt, Transkript, Analyse
- Kontinuierliche State-Updates via `emit_workflow_state()` an Frontend
- Separate Events: `transcription-progress`, `transcription-complete`, `workflow-state`, `workflow-complete`

### 5.3 Speicher-Architektur **[ERLEDIGT]**

Da Whisper (whisper.cpp, In-Process) und Gemma 4 (Ollama, separater Prozess) in
unterschiedlichen Prozessen laufen, entfaellt das manuelle Model-Swapping:

| Phase | Prozess | Modell | Speicher |
|---|---|---|---|
| Transcribing | Tauri-App | Whisper (~1.6 GB GGML) | In-Process |
| Analyzing | Ollama | Gemma 4 (~2-3 GB) | Externer Prozess |

- Kein manuelles Laden/Entladen von Modellen noetig
- Beide Modelle koennen gleichzeitig im Speicher sein (~3,5 GB gesamt)
- Ollama verwaltet Speicher eigenstaendig

### 5.4 Frontend **[ERLEDIGT]**

- "One-Click Workflow" Bereich mit Task-Auswahl (Dropdown) und Start-Button
- Phasen-Fortschrittsanzeige: Audio laden -> Transkribieren -> Analysieren
- Visuelle Phasen-Indikatoren (active/done/grey)
- Chunk-Detail-Anzeige waehrend Transkription
- Automatischer Uebergang zwischen Phasen
- Workflow kann auch manuell (Transkribieren + separat Analysieren) genutzt werden

### 5.5 Verifizierung **[ERLEDIGT]**

- `cargo check` fehlerfrei
- `cargo test` 12/12 bestanden
- `tsc --noEmit` fehlerfrei
- Frontend kompiliert mit Workflow-Features

### Neue Dateien

| Datei | Zweck |
|---|---|
| `src-tauri/src/workflow.rs` | Workflow-Modul (Phasen-Management, run_workflow Command) |

**Geschaetzter Aufwand: ~3-4 Tage -- Tatsaechlich: ~1 Sitzung**
---

## Stufe 6: Frontend / Benutzeroberflaeche **[ABGESCHLOSSEN]**

### 6.1 Hauptansicht **[ERLEDIGT]**

- Audio-Upload (Drag & Drop + Datei-Dialog)
- Echtzeit-Fortschrittsanzeige:
  - Transkriptionsfortschritt (Balken + Chunk-Index)
  - Analysefortschritt (animierter Balken)
  - Workflow-Phasen-Indikator (Audio laden -> Transkribieren -> Analysieren)
- Live-Transkript-Anzeige (Text erscheint Chunk fuer Chunk)
- Header mit Einstellungen-Button
- Statusbar mit Ollama-Status, Modellen, Audio-Info

### 6.2 Ergebnisansicht **[ERLEDIGT]**

- Tab-basierte Anzeige: Transkript / Analyse
- Transkript-Tab: editierbares Textfeld mit Zeichenanzahl
- Analyse-Tab: 8 Analyse-Buttons + Streaming-Ergebnis
- Export-Buttons in der Tab-Leiste: TXT, Markdown, JSON, SRT
- Kopieren-Button fuer Analyseergebnisse
- Zeichenanzahl-Anzeige

### 6.3 Export-Funktionen **[ERLEDIGT]**

- Neues Modul `src-tauri/src/export.rs`:
  - `export_txt()` – Transkript + Analyse als Textdatei
  - `export_markdown()` – Markdown mit Ueberschriften und Zeitstempel
  - `export_json()` – Strukturiertes JSON mit Metadaten
  - `export_srt()` – Untertitel mit Zeitstempeln (HH:MM:SS,mmm)
- Frontend: Datei-Save-Dialog via `@tauri-apps/plugin-dialog`
- `chrono` Crate fuer Zeitstempel

### 6.4 Einstellungen **[ERLEDIGT]**

- Einstellungen-Panel (aufklappbar ueber Zahnrad-Button):
  - Sprachauswahl (12 Sprachen + Automatisch)
  - Modell-Info (Whisper + Ollama)
- Sprache wird an Transkription und Workflow weitergegeben

### 6.5 UI-Politur **[ERLEDIGT]**

- Chunk-Liste als aufklappbares `<details>` Element
- Kompakte Chunk-Status-Anzeige (OK/.../-)
- Error-Banner statt rohem Text
- Status-Punkte mit Animation
- Dark Mode komplett unterstuetzt
- Responsives Layout

### 6.6 Verifizierung **[ERLEDIGT]**

- `cargo check` fehlerfrei
- `cargo test` 16/16 bestanden (inkl. 4 Export-Tests)
- `tsc --noEmit` fehlerfrei
- Dark/Light-Mode funktioniert

### Neue Dateien

| Datei | Zweck |
|---|---|
| `src-tauri/src/export.rs` | Export-Modul (TXT, MD, JSON, SRT) |

### Neue Abhaengigkeit

| Crate | Zweck |
|---|---|
| `chrono` v0.4 | Zeitstempel fuer Exporte |

**Geschaetzter Aufwand: ~3-5 Tage -- Tatsaechlich: ~1 Sitzung**
---

## Stufe 7: Optimierung & Haertung **[ABGESCHLOSSEN]**

### 7.1 Warnungen und Code-Qualitaet **[ERLEDIGT]**

- Alle Compiler-Warnungen bereinigt (unused imports, unused variables)
- 0 Warnungen bei `cargo check`
- 0 TypeScript-Fehler bei `tsc --noEmit`

### 7.2 Fehlerbehandlung & Robustheit **[ERLEDIGT]**

- **Ollama-Verfuegbarkeitspruefung beim App-Start**:
  - Async-Check im `setup()` Hook von Tauri
  - `startup-warning` Event an Frontend wenn Ollama nicht erreichbar
  - Klare Fehlermeldung mit Loesungshinweis
- **Whisper-Modellvalidierung**:
  - Neuer Command `check_whisper_model()` prueft Existenz und Dateigroesse
  - Warnung beim App-Start wenn Modell fehlt
  - Frontend zeigt Whisper-Status in Statusbar (Pruefe.../Bereit/Modell fehlt)
- **Workflow Pre-Checks**:
  - Audio-Datei-Existenz vor Workflow-Start
  - Whisper-Modell-Existenz vor Transkription
  - Ollama-Erreichbarkeit vor Analyse
  - Klare Fehlermeldungen mit Loesungshinweisen
- **Frontend Startup-Warnings**:
  - Gelbe Warnungs-Banner bei fehlenden Komponenten
  - Dismiss-Button zum Schliessen
  - Transkribieren-Button deaktiviert wenn Whisper-Modell fehlt
  - Workflow-Button deaktiviert wenn Ollama oder Whisper fehlt

### 7.3 Performance-Optimierung **[ERLEDIGT]**

- **Whisper Thread-Anzahl**: Automatisch an CPU-Kerne angepasst (`available_parallelism`), max. 8 Threads
- **Ollama HTTP-Client**: Connection-Pooling mit `OnceLock<Client>`:
  - Pool: max 2 idle Connections pro Host
  - Idle-Timeout: 30s
  - Connect-Timeout: 5s
  - Kein wiederholter Client-Aufbau pro Request
- **Audio-Pipeline**:
  - Dateigroessen-Limit: 2 GB
  - Dauer-Limit: 4 Stunden
  - Mindestdauer: 0.1s
  - Format-Warnung bei unbekannten Dateiendungen

### 7.4 Input-Validierung **[ERLEDIGT]**

- **Audio-Validierung** (`audio.rs`):
  - Datei-Existenz-Check
  - Format-Check (8 unterstuetzte Endungen)
  - Dateigroesse max. 2 GB
  - Audiodauer max. 4 Stunden
  - Mindestdauer 0.1s
- **Analyse-Validierung** (`ollama.rs`):
  - Leeres Transkript abgefangen
  - Maximallaenge 500.000 Zeichen
  - Klare Fehlermeldungen
- **Export-Validierung** (`export.rs`):
  - SRT-Export nur mit Chunk-Daten
  - Pfad-Validierung

### 7.5 Logging & Diagnose **[ERLEDIGT]**

- `tracing_subscriber` mit `env-filter` Feature initialisiert
- Log-Level: `info` (Default), steuerbar via `RUST_LOG` Umgebungsvariable
- Strukturiertes Logging fuer alle Operationen:
  - App-Start mit Komponenten-Status
  - Audio-Laden mit Metadaten
  - Chunk-Erstellung mit Fortschritt
  - Whisper-Inferenz mit Thread-Anzahl
  - Ollama-Analyse mit Task und Zeichenanzahl
  - Workflow-Phasen-Wechsel
  - Fehler mit Kontext

### 7.6 Verifizierung **[ERLEDIGT]**

- `cargo check`: 0 Warnungen, 0 Fehler
- `cargo test`: 16/16 bestanden
- `tsc --noEmit`: 0 Fehler
- `cargo build`: erfolgreich
- Alle Frontend-Komponenten kompilieren
- Dark Mode CSS komplett

### 7.7 Frontend-Logging & Fehlerbehandlung **[ERLEDIGT]**

- **Konsolen-Logging** fuer alle Operationen (`console.log`, `console.warn`, `console.error`)
- **Error-Timeout**: Fehlerbanner verschwindet automatisch nach 15s
- **Ollama-Auto-Reconnect**: Wenn Ollama nicht erreichbar, wird alle 30s erneut geprueft
- **Whisper-Status-Logging**: Modell-Pfad und Groesse werden geloggt

### 7.8 Ollama-Speichermanagement **[ERLEDIGT]**

- `keep_alive` Parameter in API-Requests konfiguriert (Default: `"5m"`)
- Ollama behaelt das Modell 5 Minuten nach letzter Anfrage im Speicher
- Danach wird es automatisch entladen -> Speicher wird freigegeben
- Konfigurierbar ueber `DEFAULT_KEEP_ALIVE` Konstante in `ollama.rs`

### Neue/Geaenderte Dateien

| Datei | Aenderung |
|---|---|
| `src-tauri/src/lib.rs` | Startup-Checks, Whisper-Validierung, startup-warning Events |
| `src-tauri/src/main.rs` | Logging-Initialisierung (tracing_subscriber) |
| `src-tauri/src/audio.rs` | Input-Validierung, Dateigroesse/Dauer-Limits |
| `src-tauri/src/whisper.rs` | Dynamische Thread-Anzahl |
| `src-tauri/src/ollama.rs` | Connection-Pooling, Input-Validierung, keep_alive |
| `src-tauri/src/workflow.rs` | Pre-Checks vor Workflow-Start |
| `src-tauri/Cargo.toml` | tracing-subscriber env-filter Feature |
| `src/App.tsx` | Whisper-Status, Startup-Warnings, canTranscribe, Logging, Auto-Reconnect, Error-Timeout |
| `src/styles.css` | Startup-Warnings Styling, Dark Mode |

**Geschaetzter Aufwand: ~2-3 Tage -- Tatsaechlich: ~1 Sitzung**
---

## Ressourcen-Budget (16 GB Laptop)

| Komponente | Geschätzter Verbrauch |
|---|---|
| Betriebssystem + Desktop | ~3-4 GB |
| Tauri-App (Frontend + Rust) | ~200-300 MB |
| Whisper Large-v3-Turbo (GGML, whisper.cpp) | ~1.6 GB (nur in Phase 1, In-Process) |
| Gemma 4 E4B (4-Bit, Ollama) | ~2-3 GB (Ollama-Prozess, wird bei Bedarf geladen) |
| Audio-Verarbeitung + Puffer | ~200-500 MB |
| **Gesamt Phase 1** | **~5-6 GB** |
| **Gesamt Phase 2** | **~6-8 GB** |

Hinweis: Da Ollama als separater Prozess laeuft, wird Gemma-4-Speicher
automatisch verwaltet. Die `keep_alive` Einstellung (Default: 5m) steuert,
wie lange Ollama das Modell nach der letzten Anfrage im Speicher behaelt.
Dies ist in den API-Requests konfiguriert (`DEFAULT_KEEP_ALIVE = "5m"`).

---

## Zeitplan

| Stufe | Inhalt | Geschätzter Aufwand |
|---|---|---|
| 1 | Projekt-Infrastruktur & Build-System | ERLEDIGT (1 Sitzung) |
| 2 | Audio-Pipeline & Chunking | ERLEDIGT (1 Sitzung) |
| 3 | Whisper-Integration (whisper.cpp) | ERLEDIGT (1 Sitzung) |
| 4 | Gemma 4 E4B via Ollama | ERLEDIGT (1 Sitzung) |
| 5 | Sequenzieller Workflow | ERLEDIGT (1 Sitzung) |
| 6 | Frontend / UI | ERLEDIGT (1 Sitzung) |
| 7 | Optimierung & Haertung | ERLEDIGT (1 Sitzung) |
| **Gesamt** | | **7/7 Stufen erledigt** |

**Alle 7 Stufen erledigt.**
Urspruenglicher Plan: ~17-27 Tage. Aktueller Plan: ~14-21 Tage.
Tatsaechlich: ~7 Sitzungen fuer alle 7 Stufen.
