import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";

const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

// --- Types ---

interface AudioInfo {
  duration_secs: number;
  sample_rate: number;
  channels: number;
  total_chunks: number;
  filename: string;
  file_size: number;
}

interface ChunkTranscript {
  index: number;
  start_secs: number;
  end_secs: number;
  text: string;
  language: string;
}

interface TranscriptionResult {
  full_text: string;
  chunks: ChunkTranscript[];
  total_duration_secs: number;
  language: string;
  model: string;
}

const ANALYSIS_OPTIONS = [
  { key: "summary", label: "Zusammenfassung" },
  { key: "detailed_summary", label: "Ausführlich" },
  { key: "topics", label: "Themen" },
  { key: "actions", label: "Aktionen" },
  { key: "decisions", label: "Beschlüsse" },
  { key: "sentiment", label: "Stimmung" },
  { key: "protocol", label: "Protokoll" },
  { key: "full", label: "Vollanalyse" },
];

const LANGUAGES = [
  { code: "de", label: "Deutsch" },
  { code: "en", label: "English" },
  { code: "fr", label: "Französisch" },
  { code: "auto", label: "Automatisch" },
];

function formatDuration(secs: number): string {
  const m = Math.floor(secs / 60);
  const s = Math.floor(secs % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}

// --- Main App ---

function App() {
  const [ollamaStatus, setOllamaStatus] = useState<"checking" | "ok" | "error">("checking");
  const [ollamaModels, setOllamaModels] = useState<string[]>([]);
  const [selectedOllamaModel, setSelectedOllamaModel] = useState<string>("");
  const [whisperStatus, setWhisperStatus] = useState<"checking" | "ok" | "error">("checking");
  const [startupWarnings, setStartupWarnings] = useState<string[]>([]);

  const [audioDevices, setAudioDevices] = useState<{ name: string; is_default: boolean }[]>([]);
  const [selectedAudioDevice, setSelectedAudioDevice] = useState<string>("");

  const [audioInfo, setAudioInfo] = useState<AudioInfo | null>(null);
  const [audioLoading, setAudioLoading] = useState(false);
  const [audioError, setAudioError] = useState("");
  const [dragOver, setDragOver] = useState(false);
  const [selectedFile, setSelectedFile] = useState("");

  const [transcript, setTranscript] = useState("");
  const [transcribing, setTranscribing] = useState(false);
  const [transcriptChunks, setTranscriptChunks] = useState<ChunkTranscript[]>([]);

  const [result, setResult] = useState("");
  const [loading, setLoading] = useState(false);
  const [activeTask, setActiveTask] = useState("");
  const [streamingResult, setStreamingResult] = useState("");
  const resultRef = useRef<HTMLDivElement>(null);

  const [language, setLanguage] = useState("de");
  const [showSettings, setShowSettings] = useState(false);

  const [recording, setRecording] = useState(false);
  const [recordingTime, setRecordingTime] = useState(0);
  const recordingTimerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const [audioLevel, setAudioLevel] = useState(0);
  const [gain, setGain] = useState(1.0);
  const canvasRef = useRef<HTMLCanvasElement>(null);

  // --- Event Listeners ---

  useEffect(() => {
    if (isTauri) {
      checkOllama();
      checkWhisper();
      loadAudioDevices();
      invoke("start_monitoring").catch(() => {});
    }

    const unlistenProgress = isTauri ? listen("transcription-progress", (event) => {
      const data = event.payload as any;
      if (data.accumulated_text) setTranscript(data.accumulated_text);
    }) : Promise.resolve(() => {});

    const unlistenComplete = isTauri ? listen("transcription-complete", (event) => {
      const data = event.payload as TranscriptionResult;
      setTranscript(data.full_text);
      setTranscriptChunks(data.chunks);
      setTranscribing(false);
    }) : Promise.resolve(() => {});

    const unlistenToken = isTauri ? listen("analysis-token", (event) => {
      const data = event.payload as any;
      setStreamingResult((prev) => prev + data.token);
    }) : Promise.resolve(() => {});

    const unlistenAnalysisComplete = isTauri ? listen("analysis-complete", (event) => {
      const data = event.payload as any;
      setResult(data.result);
      setLoading(false);
      setActiveTask("");
    }) : Promise.resolve(() => {});

    const unlistenAudioLevel = isTauri ? listen("audio-level", (event) => {
      const data = event.payload as any;
      setAudioLevel(data.level);
    }) : Promise.resolve(() => {});

    return () => {
      unlistenProgress.then((fn) => fn());
      unlistenComplete.then((fn) => fn());
      unlistenToken.then((fn) => fn());
      unlistenAnalysisComplete.then((fn) => fn());
      unlistenAudioLevel.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    if (resultRef.current) resultRef.current.scrollTop = resultRef.current.scrollHeight;
  }, [streamingResult, result]);

  // VU-Meter Canvas
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const w = canvas.width;
    const h = canvas.height;
    ctx.clearRect(0, 0, w, h);
    const barWidth = w * audioLevel;
    const gradient = ctx.createLinearGradient(0, 0, w, 0);
    gradient.addColorStop(0, "rgba(212, 175, 55, 0.3)");
    gradient.addColorStop(1, "rgba(212, 175, 55, 1)");
    ctx.fillStyle = gradient;
    ctx.beginPath();
    ctx.roundRect(0, 0, barWidth, h, h / 2);
    ctx.fill();
  }, [audioLevel]);

  // --- Actions ---

  async function checkOllama() {
    try {
      const res = (await invoke("check_ollama_status")) as any;
      setOllamaStatus("ok");
      const names = (res?.models || []).map((m: any) => m.name);
      setOllamaModels(names);
      if (names.length > 0 && !selectedOllamaModel) setSelectedOllamaModel(names[0]);
    } catch { setOllamaStatus("error"); }
  }

  async function checkWhisper() {
    try {
      const res = (await invoke("check_whisper_model")) as any;
      setWhisperStatus(res.valid ? "ok" : "error");
    } catch { setWhisperStatus("error"); }
  }

  async function loadAudioDevices() {
    try {
      const devices = (await invoke("list_audio_devices")) as any[];
      setAudioDevices(devices);
      const def = devices.find((d) => d.is_default);
      if (def) setSelectedAudioDevice(def.name);
      else if (devices.length > 0) setSelectedAudioDevice(devices[0].name);
    } catch {}
  }

  async function handleAudioFile(path: string) {
    setAudioLoading(true);
    setAudioError("");
    setAudioInfo(null);
    setSelectedFile(path);
    try {
      const res = (await invoke("prepare_chunks", { path })) as any;
      setAudioInfo(res.audio_info);
    } catch (e) { setAudioError(`Ladefehler: ${e}`); }
    finally { setAudioLoading(false); }
  }

  async function startTranscription() {
    if (!selectedFile) return;
    setTranscribing(true);
    setTranscript("");
    setResult("");
    setStreamingResult("");
    try {
      const res = (await invoke("transcribe_audio", { path: selectedFile, language })) as TranscriptionResult;
      setTranscript(res.full_text);
      setTranscriptChunks(res.chunks);
    } catch (e) { setAudioError(`Fehler: ${e}`); }
    finally { setTranscribing(false); }
  }

  async function startRecording() {
    try {
      setRecording(true);
      setRecordingTime(0);
      setTranscript("");
      await invoke("start_recording");
      recordingTimerRef.current = setInterval(() => setRecordingTime(v => v + 1), 1000);
    } catch (e) { setRecording(false); setAudioError(`Fehler: ${e}`); }
  }

  async function stopRecording() {
    setRecording(false);
    if (recordingTimerRef.current) clearInterval(recordingTimerRef.current);
    try {
      const res = (await invoke("stop_recording")) as any;
      setAudioInfo({
        duration_secs: res.duration_secs,
        sample_rate: 16000, channels: 1, total_chunks: 0,
        filename: res.filename || "aufnahme.wav", file_size: res.file_size || 0
      });
      setSelectedFile(res.path);
      setTimeout(() => startTranscription(), 500);
    } catch (e) { setAudioError(`Fehler: ${e}`); }
  }

  async function runAnalysis(task: string) {
    if (!transcript.trim()) return;
    setLoading(true);
    setActiveTask(task);
    setResult("");
    setStreamingResult("");
    try { await invoke("analyze_transcript_stream", { transcript, task }); }
    catch (e) { setResult(`Fehler: ${e}`); setLoading(false); setActiveTask(""); }
  }

  async function handleExport(format: string) {
    const audioName = audioInfo?.filename || "audio";
    const baseName = audioName.replace(/\.[^.]+$/, "");
    const path = await save({ defaultPath: `${baseName}.${format}` });
    if (!path) return;
    try {
      if (format === "srt") {
        const data = JSON.stringify(transcriptChunks.map(c => ({ start_secs: c.start_secs, end_secs: c.end_secs, text: c.text })));
        await invoke("export_srt_file", { chunksJson: data, path });
      } else {
        await invoke("export_result", { transcript, analysis: result || null, audioName, format, path });
      }
    } catch (e) { setAudioError(`Export-Fehler: ${e}`); }
  }

  // --- Render ---

  return (
    <div className="app-container">
      {/* Header Slim */}
      <header className="app-header">
        <h1>Diktat</h1>
        <div className="header-actions">
          <div className="status-indicators">
            <div className="status-dot-item"><span className={`dot ${ollamaStatus}`} /> Ollama</div>
            <div className="status-dot-item"><span className={`dot ${whisperStatus}`} /> Whisper</div>
          </div>
          <button className="icon-btn-gold" onClick={() => setShowSettings(true)}>&#9881;</button>
        </div>
      </header>

      {audioError && <div className="error-banner-tiny">{audioError}</div>}

      {/* Input Section Stack */}
      <section className="section-card">
        {!recording ? (
          <div className="input-split">
            <div className="input-option" onClick={startRecording}>
              <span className="icon">🎙️</span>
              <h3>Aufnahme</h3>
              <button className="btn-record-gold">Start</button>
            </div>
            <div 
              className={`input-option ${dragOver ? 'drag-over' : ''}`}
              onDrop={(e) => { e.preventDefault(); setDragOver(false); const f = e.dataTransfer.files; if (f.length) handleAudioFile((f[0] as any).path || f[0].name); }}
              onDragOver={(e) => { e.preventDefault(); setDragOver(true); }}
              onDragLeave={() => setDragOver(false)}
              onClick={async () => { const s = await open({ multiple: false }); if (s) handleAudioFile(s as string); }}
            >
              <span className="icon">📂</span>
              <h3>Upload</h3>
              <div className="drop-zone-compact">
                {audioLoading ? "Lade..." : audioInfo ? audioInfo.filename : "Datei wählen"}
              </div>
            </div>
          </div>
        ) : (
          <div className="recording-live">
            <div className="recording-timer-compact">{formatDuration(recordingTime)}</div>
            <div className="vu-meter-slim">
              <canvas ref={canvasRef} width={400} height={12} />
            </div>
            <button className="btn-record-gold recording" onClick={stopRecording} style={{width: 'auto'}}>Stop</button>
          </div>
        )}
        {audioInfo && !transcribing && !recording && (
          <button className="btn-record-gold" onClick={startTranscription}>Transkription starten</button>
        )}
      </section>

      {/* Processing Indicator */}
      {transcribing && (
        <div className="processing-inline">
          <div className="loader-tiny" />
          <span className="processing-text-tiny">Transkribiere... {transcript.length} Zeichen erfasst</span>
        </div>
      )}

      {/* Result Section */}
      <section className="section-card">
        <div className="section-title">
          <span>Ergebnis</span>
          <span className="subtitle" style={{fontSize: '0.7rem'}}>{transcript.length} Zeichen</span>
        </div>
        <div className="result-stack">
          <textarea 
            className="transcript-area-compact" 
            value={transcript} 
            onChange={e => setTranscript(e.target.value)}
            placeholder="Warte auf Transkription..."
          />
          
          <div className="analysis-buttons-compact">
            {ANALYSIS_OPTIONS.map(opt => (
              <button 
                key={opt.key} 
                className={`analysis-chip ${activeTask === opt.key ? 'active' : ''}`}
                onClick={() => runAnalysis(opt.key)}
                disabled={loading || !transcript.trim()}
              >
                {opt.label}
              </button>
            ))}
          </div>

          {(result || streamingResult || loading) && (
            <div className="analysis-result-compact" ref={resultRef}>
              {loading ? streamingResult || "Analysiere..." : result}
            </div>
          )}

          <div className="export-bar-compact">
            <div style={{display: 'flex', gap: '0.4rem'}}>
              <button className="btn-export-tiny" onClick={() => handleExport('txt')}>TXT</button>
              <button className="btn-export-tiny" onClick={() => handleExport('md')}>MD</button>
              {transcriptChunks.length > 0 && <button className="btn-export-tiny" onClick={() => handleExport('srt')}>SRT</button>}
            </div>
            <button className="btn-export-tiny" onClick={() => { setTranscript(""); setResult(""); setAudioInfo(null); }}>Neu</button>
          </div>
        </div>
      </section>

      {/* Settings Panel */}
      <div className={`settings-compact ${showSettings ? 'open' : ''}`}>
        <button className="settings-close-btn" onClick={() => setShowSettings(false)}>✕ Schließen</button>
        <h2 className="serif-text" style={{fontSize: '1.2rem'}}>Einstellungen</h2>
        
        <div className="settings-group-compact">
          <label>Sprache</label>
          <select className="settings-select-compact" value={language} onChange={e => setLanguage(e.target.value)}>
            {LANGUAGES.map(l => <option key={l.code} value={l.code}>{l.label}</option>)}
          </select>
        </div>

        <div className="settings-group-compact">
          <label>Mikrofon</label>
          <select className="settings-select-compact" value={selectedAudioDevice} onChange={e => { setSelectedAudioDevice(e.target.value); invoke("set_audio_device", { deviceName: e.target.value }); }}>
            {audioDevices.map(d => <option key={d.name} value={d.name}>{d.name}</option>)}
          </select>
        </div>

        <div className="settings-group-compact">
          <label>KI-Modell</label>
          <select className="settings-select-compact" value={selectedOllamaModel} onChange={e => { setSelectedOllamaModel(e.target.value); invoke("set_ollama_model", { model: e.target.value }); }}>
            {ollamaModels.map(m => <option key={m} value={m}>{m}</option>)}
          </select>
        </div>

        <div className="settings-group-compact">
          <label>Gain: {gain.toFixed(1)}x</label>
          <input type="range" min="0.1" max="5.0" step="0.1" value={gain} onChange={e => { setGain(parseFloat(e.target.value)); invoke("set_recording_gain", { gain: parseFloat(e.target.value) }); }} style={{accentColor: 'var(--gold-accent)'}} />
        </div>
      </div>
    </div>
  );
}

export default App;
