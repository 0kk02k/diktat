# Diktat

Lokale Transkriptions-App mit KI-gestuetzter Textanalyse. Alles laeuft offline auf deinem Rechner -- keine Cloud, keine Datenabhaengigkeit.

## Was macht Diktat?

1. **Audio aufnehmen oder laden** -- WAV, MP3, FLAC, OGG, M4A, AAC
2. **Transkribieren** -- Whisper Large-v3-Turbo erkennt Sprache lokal
3. **Analysieren** -- Gemma 4 via Ollama analysiert das Transkript in 8 verschiedenen Modi
4. **Exportieren** -- TXT, Markdown, JSON oder SRT (Untertitel)

Alle Modelle laufen lokal. Es werden keine Daten an externe Server gesendet.

## Architektur

| Komponente | Technologie | Zweck |
|---|---|---|
| **Frontend** | React 19 + TypeScript + Vite | Benutzeroberflaeche |
| **Backend** | Tauri 2 (Rust) | Native App-Shell, Audio-Verarbeitung |
| **Spracherkennung** | Whisper Large-v3-Turbo via whisper.cpp | Audio-to-Text |
| **Textanalyse** | Gemma 4 via Ollama | Transkript-Analyse |
| **Audio-Processing** | symphonia + rubato | Dekodierung, Resampling, Chunking |
| **Audio-Aufnahme** | cpal | Natives Mikrofon-Capture |

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

## Audio-Aufnahme

### Echtzeit-Pegelanzeige (VU-Meter)

Beim App-Start wird automatisch ein Audio-Monitoring gestartet, das den Mikrofoneingang in Echtzeit anzeigt:

- **Farbiger Balken** von -60 dB bis 0 dB (gruen/gelb/orange/rot)
- **Verlaufskurve** der letzten Messungen
- **dB-Anzeige** als Zahlenwert

### Lautstaerkeregelung (Gain)

Ein Slider (0.1x bis 5.0x) steuert die Eingangsverstaerkung. Wenn das Mikrofon zu leise ist, kann der Gain erhoeht werden. Die Aenderung wirkt sich sofort auf Aufnahme und Pegelanzeige aus.

### Speicheroptimierung

Aufnahmen werden automatisch auf 16 kHz resampled bevor sie als WAV gespeichert werden. Das reduziert die Dateigroesse um ~96% (z.B. 2.4s Aufnahme = 76 KB statt 1.8 MB). Whisper benoetigt sowieso 16 kHz, daher kein Qualitaetsverlust.

## Voraussetzungen

- **Betriebssystem:** Linux (x86_64) -- Windows/macOS erfordert separates Kompilieren
- **RAM:** Mindestens 8 GB, empfohlen 16 GB
- **Festplatte:** ca. 4 GB fuer Modelle + App
- **Ollama:** Installiert und laufend (`ollama serve`)
- **Mikrofon:** Funktionierendes Mikrofon (3.5mm, USB oder Bluetooth)

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

### 3. Ollama und Gemma 4 einrichten

```bash
# Ollama installieren
curl -fsSL https://ollama.com/install.sh | sh

# Modell herunterladen
ollama pull gemma4

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
│   ├── App.tsx                 # Hauptkomponente (1090 Zeilen)
│   ├── main.tsx                # React-Einstiegspunkt
│   ├── styles.css              # Styling inkl. Dark Mode, VU-Meter, Gain-Slider
│   └── vite-env.d.ts           # Vite-Typen
├── src-tauri/                  # Backend (Rust/Tauri)
│   ├── src/
│   │   ├── lib.rs              # Tauri-App-Einstieg, Commands, Startup-Checks
│   │   ├── main.rs             # Binary-Entry-Point, Logging-Init
│   │   ├── audio.rs            # Audio-Dekodierung, Resampling, Chunking
│   │   ├── recording.rs        # Mikrofon-Aufnahme (cpal), Monitoring, Gain, VU-Meter
│   │   ├── whisper.rs          # Whisper-Integration (whisper.cpp)
│   │   ├── ollama.rs           # Ollama REST-API-Client, 8 Analyse-Tasks
│   │   ├── workflow.rs         # Sequenzieller Workflow (Transkription + Analyse)
│   │   └── export.rs           # Export: TXT, MD, JSON, SRT
│   ├── Cargo.toml              # Rust-Abhaengigkeiten
│   ├── tauri.conf.json         # Tauri-Konfiguration
│   └── icons/                  # App-Icons
├── models/                     # Whisper-Modell (nicht im Repo)
│   └── ggml-large-v3-turbo.bin # 1.6 GB, separat herunterladen
├── recordings/                 # Aufnahmen (automatisch erstellt)
├── index.html                  # HTML-Entry
├── package.json                # npm-Abhaengigkeiten
├── vite.config.ts              # Vite-Konfiguration
└── tsconfig.json               # TypeScript-Konfiguration
```

## Workflow

```
Audio-Datei oder Mikrofon-Aufnahme
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
Gemma 4 via Ollama: Transkript analysieren (Streaming)
    │
    ▼
Ergebnis anzeigen & exportieren
```

## Fehlerbehebung

### Mikrofon zeigt keinen Ausschlag

1. Mikrofon anschliessen und pruefen: `arecord -d 3 -f S16_LE -r 16000 /tmp/test.wav && python3 -c "import wave; w=wave.open('/tmp/test.wav','rb'); print(w.readframes(w.getnframes())[:100])"`
2. PipeWire-Quelle pruefen: `pactl list sources short`
3. Eingangslautstaerke pruefen: `amixer sget 'Capture',0`
4. In der App den Gain-Slider auf 2-5x erhoehen

### Ollama nicht erreichbar

```bash
ollama serve    # Im Hintergrund starten
ollama list     # Verfuegbare Modelle anzeigen
```

### Whisper halluziniert bei Stille

Whisper kann bei leisen oder leeren Audioabschnitten halluzinieren ("Vielen Dank", "Thank you"). Die Anti-Halluzinations-Parameter sind bereits konfiguriert (`no_speech_thold`, `logprob_thold`, `initial_prompt`).

## Speicherbedarf

| Komponente | Verbrauch |
|---|---|
| Betriebssystem + Desktop | ~3-4 GB |
| Tauri-App (Frontend + Rust) | ~200-300 MB |
| Whisper Large-v3-Turbo (whisper.cpp) | ~1.6 GB (In-Process) |
| Gemma 4 (Ollama) | ~9-10 GB (separater Prozess, auto-entladen nach 5 Min) |
| Audio-Verarbeitung + Puffer | ~200-500 MB |
| **Gesamt** | **~12-16 GB** |

## Tests

```bash
# Rust-Tests
cd src-tauri && cargo test

# TypeScript-Typecheck
npx tsc --noEmit

# Rust-Kompilierungscheck
cd src-tauri && cargo check
```

## Lizenz

Privatprojekt. Alle verwendeten Modelle und Bibliotheken unterliegen ihren jeweiligen Lizenzen.
