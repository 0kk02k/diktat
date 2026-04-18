# Diktat

Lokale Transkriptions-App mit KI-gestuetzter Textanalyse. Alles laeuft offline auf deinem Rechner -- keine Cloud, keine Datenabhaengigkeit.

## Was macht Diktat?

1. **Audio aufnehmen oder laden** -- WAV, MP3, FLAC, OGG, M4A, AAC
2. **Transkribieren** -- Whisper Large-v3-Turbo erkennt Sprache lokal
3. **Analysieren** -- Gemma 3:4B analysiert das Transkript in 8 verschiedenen Modi
4. **Exportieren** -- TXT, Markdown, JSON oder SRT (Untertitel)

Alle Modelle laufen lokal. Es werden keine Daten an externe Server gesendet.

## Architektur

| Komponente | Technologie | Zweck |
|---|---|---|
| **Frontend** | React 19 + TypeScript + Vite | Benutzeroberflaeche |
| **Backend** | Tauri 2 (Rust) | Native App-Shell, Audio-Verarbeitung |
| **Spracherkennung** | Whisper Large-v3-Turbo via whisper.cpp | Audio-to-Text |
| **Textanalyse** | Gemma 3:4B via Ollama | Transkript-Analyse |
| **Audio-Processing** | symphonia + rubato | Dekodierung, Resampling, Chunking |

## Analyse-Modi

| Modus | Beschreibung |
|---|---|
| Zusammenfassung | Kurze, praegnante Zusammenfassung (3-5 Absaetze) |
| Ausfuehrlich | Detaillierte Zusammenfassung mit Ueberschriften |
| Themen | Hauptthemen und Keywords mit Erklaerung |
| Aktionspunkte | To-dos mit Personen-Zuordnung |
| Beschluesse | Entscheidungen mit Verantwortlichen und Fristen |
| Stimmungsanalyse | Emotionale Tendenz und Stimmungswechsel |
| Protokoll | Strukturiertes Protokoll (Agenda, Diskussion, Beschluesse) |
| Vollanalyse | Alle Analysen kombiniert |

## Voraussetzungen

- **Betriebssystem:** Linux (x86_64) -- Windows/macOS erfordert separates Kompilieren
- **RAM:** Mindestens 8 GB, empfohlen 16 GB
- **Festplatte:** ca. 4 GB fuer Modelle + App
- **Ollama:** Installiert und laufend (`ollama serve`)

### Software-Abhaengigkeiten

| Tool | Version |
|---|---|
| Node.js | >= 18 |
| npm | >= 9 |
| Rust | >= 1.70 |
| Cargo | >= 1.70 |
| Tauri CLI | >= 2.0 |

#### Linux-Systempakete (Debian/Ubuntu)

```bash
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev \
  libjavascriptcoregtk-4.1-dev libayatana-appindicator3-dev \
  librsvg2-dev patchelf libclang-dev clang cmake build-essential
```

## Installation

### 1. Repository klonen

```bash
git clone https://github.com/0kk02k/diktat.git
cd diktat
```

### 2. Frontend-Abhaengigkeiten installieren

```bash
npm install
```

### 3. Ollama und Gemma 3:4B einrichten

```bash
# Ollama installieren
curl -fsSL https://ollama.com/install.sh | sh

# Modell herunterladen
ollama pull gemma3:4b

# Ollama starten
ollama serve
```

### 4. Whisper-Modell herunterladen

```bash
mkdir -p models
wget -O models/ggml-large-v3-turbo.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin
```

### 5. App starten (Entwicklungsmodus)

```bash
npx tauri dev
```

### 6. Produktiv-Build erstellen

```bash
npx tauri build
```

Erstellt DEB, RPM und AppImage unter `src-tauri/target/release/bundle/`.

## Projektstruktur

```
diktat/
├── src/                        # Frontend (React/TypeScript)
│   ├── App.tsx                 # Hauptkomponente (875 Zeilen)
│   ├── main.tsx                # React-Einstiegspunkt
│   ├── styles.css              # Styling inkl. Dark Mode
│   └── vite-env.d.ts           # Vite-Typen
├── src-tauri/                  # Backend (Rust/Tauri)
│   ├── src/
│   │   ├── lib.rs              # Tauri-App-Einstieg, Commands, Startup-Checks
│   │   ├── main.rs             # Binary-Entry-Point, Logging-Init
│   │   ├── audio.rs            # Audio-Dekodierung, Resampling, Chunking
│   │   ├── whisper.rs          # Whisper-Integration (whisper.cpp)
│   │   ├── ollama.rs           # Ollama REST-API-Client, 8 Analyse-Tasks
│   │   ├── workflow.rs         # Sequenzieller Workflow (Transkription + Analyse)
│   │   └── export.rs           # Export: TXT, MD, JSON, SRT
│   ├── Cargo.toml              # Rust-Abhaengigkeiten
│   ├── tauri.conf.json         # Tauri-Konfiguration
│   └── icons/                  # App-Icons
├── models/                     # Whisper-Modell (nicht im Repo)
│   └── ggml-large-v3-turbo.bin # 1.6 GB, separat herunterladen
├── index.html                  # HTML-Entry
├── package.json                # npm-Abhaengigkeiten
├── vite.config.ts              # Vite-Konfiguration
└── tsconfig.json               # TypeScript-Konfiguration
```

## Workflow

```
Audio-Datei
    │
    ▼
Audio laden & chunken (30s-Fenster, 1.5s Ueberlappung)
    │
    ▼
Whisper Large-v3-Turbo: Chunks sequenziell transkribieren
    │
    ▼
Ueberlappungs-Bereinigung (doppelte Woerter entfernen)
    │
    ▼
Gemma 3:4B via Ollama: Transkript analysieren (Streaming)
    │
    ▼
Ergebnis anzeigen & exportieren
```

## Speicherbedarf

| Komponente | Verbrauch |
|---|---|
| Betriebssystem + Desktop | ~3-4 GB |
| Tauri-App (Frontend + Rust) | ~200-300 MB |
| Whisper Large-v3-Turbo (whisper.cpp) | ~1.6 GB (In-Process) |
| Gemma 3:4B (Ollama) | ~2-3 GB (separater Prozess, auto-entladen nach 5 Min) |
| Audio-Verarbeitung + Puffer | ~200-500 MB |
| **Gesamt** | **~6-8 GB** |

## Tests

```bash
# Rust-Tests (16 Unit-Tests)
cd src-tauri && cargo test

# TypeScript-Typecheck
npx tsc --noEmit

# Rust-Kompilierungscheck
cd src-tauri && cargo check
```

## Lizenz

Privatprojekt. Alle verwendeten Modelle und Bibliotheken unterliegen ihren jeweiligen Lizenzen.
