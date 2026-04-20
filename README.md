# Diktat (Premium Edition)

Lokale Transkriptions-App mit KI-gestützter Textanalyse. Alles läuft offline auf deinem Rechner -- keine Cloud, keine Datenabhängigkeit. Die Premium Edition bietet ein edles Design und einen hochoptimierten Workflow.

## Was macht Diktat?

1. **Studio (Erfassung)** -- Intuitive Aufnahme über das Mikrofon oder einfacher Datei-Upload (WAV, MP3, FLAC, etc.).
2. **Engine (Verarbeitung)** -- Hochperformante Transkription mit Whisper Large-v3-Turbo ohne Ablenkungen.
3. **Dokument (Analyse & Export)** -- Edle Texteditor-Ansicht mit 8 KI-Analysemodi und professionellen Exportoptionen.

Alle Modelle laufen lokal. Es werden keine Daten an externe Server gesendet.

## Architektur

| Komponente | Technologie | Zweck |
|---|---|---|
| **Frontend** | React 19 + TypeScript + Vite | Premium UI (Playfair Display & Inter) |
| **Backend** | Tauri 2 (Rust) | Native App-Shell, Audio-Verarbeitung |
| **Spracherkennung** | Whisper Large-v3-Turbo via whisper.cpp | Audio-to-Text |
| **Textanalyse** | Ollama (Gemma 3/4 empfohlen) | Transkript-Analyse |
| **Audio-Processing** | symphonia + rubato | Dekodierung, Resampling, Chunking |
| **Audio-Aufnahme** | cpal | Natives Mikrofon-Capture |

## Der Premium Wizard-Workflow

Diktat nutzt einen geführten Workflow ("Wizard"), der dich durch den Prozess leitet:

### Phase 1: Das Studio (Input)
*Fokus auf Erfassung.*
- **Aufnahme:** Eleganter Timer und ein organisches, glühendes VU-Meter visualisieren deine Aufnahme.
- **Upload:** Datei einfach per Drag & Drop in die edle Upload-Zone ziehen.

### Phase 2: Die Engine (Processing)
*Fokus auf Leistung.*
- Ein minimalistischer Screen zeigt den Fortschritt der Transkription. Whisper verarbeitet das Audio lokal in Rekordzeit.

### Phase 3: Das Dokument (Ergebnis)
*Fokus auf Textarbeit.*
- **Editor:** Ein hochwertiger, editierbarer Bereich für dein Transkript.
- **Analyse:** Über eine schlanke Seitenleiste können 8 verschiedene KI-Analysen (Zusammenfassungen, Aktionspunkte, Protokolle etc.) gestartet werden.
- **Export:** Ein Klick exportiert dein Ergebnis als TXT, Markdown, JSON oder SRT.

## Design-Merkmale

- **Premium Dark Mode:** Tiefes Anthrazit kombiniert mit edlen Gold- und Kupferakzenten.
- **Elegante Typografie:** Playfair Display für Überschriften und Inter für maximale Lesbarkeit im Fließtext.
- **Slide-over Settings:** Einstellungen für Mikrofon, KI-Modell und Sprache gleiten dezent von rechts ein.

## Analyse-Modi

| Modus | Beschreibung |
|---|---|
| Zusammenfassung | Kurze, prägnante Zusammenfassung (3-5 Absätze) |
| Ausführlich | Detaillierte Zusammenfassung mit Überschriften |
| Themen | Hauptthemen und Keywords mit Erklärung |
| Aktionspunkte | To-dos mit Personen-Zuordnung |
| Beschlüsse | Entscheidungen mit Verantwortlichen und Fristen |
| Stimmungsanalyse | Emotionale Tendenz und Stimmungswechsel |
| Protokoll | Strukturiertes Protokoll (Agenda, Diskussion, Beschlüsse) |
| Vollanalyse | Alle Analysen kombiniert |

## Voraussetzungen

- **RAM:** Mindestens 8 GB, empfohlen 16 GB.
- **Ollama:** Installiert und laufend (`ollama serve`).
- **Whisper-Modell:** `ggml-large-v3-turbo.bin` im `models/` Ordner.

## Installation

```bash
# 1. Repository klonen
git clone https://github.com/0kk02k/diktat.git && cd diktat

# 2. Frontend installieren
npm install

# 3. Whisper-Modell laden
mkdir -p models
wget -O models/ggml-large-v3-turbo.bin https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin

# 4. Starten
npx tauri dev
```

## Projektstruktur

- `src/App.tsx`: Wizard-Logik und Premium-Komponenten.
- `src/styles.css`: Dark & Gold Theme Definitionen.
- `src-tauri/src/`: Performantes Rust-Backend für Audio und Inferenz.

## Lizenz

Privatprojekt. Alle verwendeten Modelle und Bibliotheken unterliegen ihren jeweiligen Lizenzen.
