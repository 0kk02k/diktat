import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";

// --- Types ---

interface AudioInfo {
  duration_secs: number;
  sample_rate: number;
  channels: number;
  total_chunks: number;
  filename: string;
  file_size: number;
}

interface ChunkMeta {
  index: number;
  start_secs: number;
  end_secs: number;
  sample_count: number;
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

interface AnalysisOption {
  key: string;
  label: string;
  icon: string;
}

type WorkflowPhase = "Idle" | "LoadingAudio" | "Transcribing" | "Analyzing" | "Complete" | "Error";

interface WorkflowState {
  phase: WorkflowPhase;
  total_chunks: number;
  chunks_done: number;
  transcript: string | null;
  analysis_result: string | null;
  error: string | null;
}

// --- Constants ---

const ANALYSIS_OPTIONS: AnalysisOption[] = [
  { key: "summary", label: "Zusammenfassung", icon: "\u{1F4DD}" },
  { key: "detailed_summary", label: "Ausfuehrlich", icon: "\u{1F4CB}" },
  { key: "topics", label: "Themen", icon: "\u{1F3F7}\u{FE0F}" },
  { key: "actions", label: "Aktionspunkte", icon: "\u{2705}" },
  { key: "decisions", label: "Beschluesse", icon: "\u{1F4CC}" },
  { key: "sentiment", label: "Stimmung", icon: "\u{1F3AD}" },
  { key: "protocol", label: "Protokoll", icon: "\u{1F4C4}" },
  { key: "full", label: "Vollanalyse", icon: "\u{1F50D}" },
];

const LANGUAGES = [
  { code: "de", label: "Deutsch" },
  { code: "en", label: "English" },
  { code: "fr", label: "Franzoesisch" },
  { code: "es", label: "Spanisch" },
  { code: "it", label: "Italienisch" },
  { code: "pt", label: "Portugiesisch" },
  { code: "nl", label: "Niederlaendisch" },
  { code: "pl", label: "Polnisch" },
  { code: "ru", label: "Russisch" },
  { code: "zh", label: "Chinesisch" },
  { code: "ja", label: "Japanisch" },
  { code: "auto", label: "Automatisch" },
];

// --- Helpers ---

function formatDuration(secs: number): string {
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = Math.floor(secs % 60);
  if (h > 0) return `${h}:${m.toString().padStart(2, "0")}:${s.toString().padStart(2, "0")}`;
  return `${m}:${s.toString().padStart(2, "0")}`;
}

function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

// --- Main App ---

function App() {
  // Connection
  const [ollamaStatus, setOllamaStatus] = useState<"checking" | "ok" | "error">("checking");
  const [ollamaModels, setOllamaModels] = useState<string[]>([]);
  const [whisperStatus, setWhisperStatus] = useState<"checking" | "ok" | "error">("checking");
  const [startupWarnings, setStartupWarnings] = useState<string[]>([]);

  // Audio
  const [audioInfo, setAudioInfo] = useState<AudioInfo | null>(null);
  const [chunks, setChunks] = useState<ChunkMeta[]>([]);
  const [audioLoading, setAudioLoading] = useState(false);
  const [audioError, setAudioError] = useState("");
  const [dragOver, setDragOver] = useState(false);
  const [selectedFile, setSelectedFile] = useState("");

  // Transcription
  const [transcript, setTranscript] = useState("");
  const [transcribing, setTranscribing] = useState(false);
  const [transcriptionProgress, setTranscriptionProgress] = useState(0);
  const [currentChunkIndex, setCurrentChunkIndex] = useState(-1);
  const [totalChunks, setTotalChunks] = useState(0);
  const [chunkStatuses, setChunkStatuses] = useState<Record<number, "pending" | "transcribing" | "done" | "error">>({});
  const [transcriptChunks, setTranscriptChunks] = useState<ChunkTranscript[]>([]);

  // Analysis
  const [result, setResult] = useState("");
  const [loading, setLoading] = useState(false);
  const [activeTask, setActiveTask] = useState("");
  const [streamingResult, setStreamingResult] = useState("");
  const resultRef = useRef<HTMLDivElement>(null);

  // Workflow
  const [workflowPhase, setWorkflowPhase] = useState<WorkflowPhase>("Idle");
  const [workflowRunning, setWorkflowRunning] = useState(false);
  const [selectedWorkflowTask, setSelectedWorkflowTask] = useState<string>("summary");

  // Settings
  const [language, setLanguage] = useState("de");
  const [showSettings, setShowSettings] = useState(false);

  // Tab for results area
  const [activeTab, setActiveTab] = useState<"transcript" | "analysis">("transcript");

  // --- Event Listeners ---

  useEffect(() => {
    checkOllama();
    checkWhisper();

    // Startup-Warnings vom Backend
    const unlistenStartupWarning = listen("startup-warning", (event: any) => {
      const data = event.payload as { component: string; message: string };
      setStartupWarnings((prev) => [...prev, data.message]);
    });

    const unlistenProgress = listen("transcription-progress", (event) => {
      const data = event.payload as any;
      setTranscriptionProgress(data.progress_percent);
      setCurrentChunkIndex(data.chunk_index);

      setChunkStatuses((prev) => {
        const updated = { ...prev };
        for (let i = 0; i <= data.chunk_index; i++) {
          updated[i] = "done";
        }
        if (data.chunk_index + 1 < data.total_chunks) {
          updated[data.chunk_index + 1] = "transcribing";
        }
        return updated;
      });

      if (data.current_text) {
        setTranscript((prev) => prev ? prev + " " + data.current_text : data.current_text);
      }
    });

    const unlistenComplete = listen("transcription-complete", (event) => {
      const data = event.payload as TranscriptionResult;
      setTranscript(data.full_text);
      setTranscriptChunks(data.chunks);
      setTranscribing(false);
      setTranscriptionProgress(100);
      setChunkStatuses((prev) => {
        const updated = { ...prev };
        for (let i = 0; i < data.chunks.length; i++) {
          updated[i] = "done";
        }
        return updated;
      });
      setActiveTab("transcript");
    });

    const unlistenToken = listen("analysis-token", (event) => {
      const data = event.payload as any;
      setStreamingResult((prev) => prev + data.token);
    });

    const unlistenAnalysisComplete = listen("analysis-complete", (event) => {
      const data = event.payload as any;
      setResult(data.result);
      setLoading(false);
      setActiveTask("");
      setActiveTab("analysis");
    });

    const unlistenWorkflowState = listen("workflow-state", (event) => {
      const data = event.payload as WorkflowState;
      setWorkflowPhase(data.phase);
      if (data.transcript) setTranscript(data.transcript);
      if (data.analysis_result) {
        setStreamingResult(data.analysis_result);
        setResult(data.analysis_result);
      }
      if (data.chunks_done > 0) {
        setTranscriptionProgress((data.chunks_done / data.total_chunks) * 100);
        setCurrentChunkIndex(data.chunks_done - 1);
        setChunkStatuses((prev) => {
          const updated = { ...prev };
          for (let i = 0; i < data.chunks_done; i++) {
            updated[i] = "done";
          }
          if (data.chunks_done < data.total_chunks) {
            updated[data.chunks_done] = "transcribing";
          }
          return updated;
        });
      }
    });

    const unlistenWorkflowComplete = listen("workflow-complete", (event) => {
      const data = event.payload as any;
      setTranscript(data.transcript);
      setResult(data.analysis);
      setStreamingResult(data.analysis);
      setWorkflowRunning(false);
      setWorkflowPhase("Complete");
      setTranscribing(false);
      setTranscriptionProgress(100);
      setLoading(false);
      setActiveTask("");
      setActiveTab("analysis");
    });

    return () => {
      unlistenProgress.then((fn) => fn());
      unlistenComplete.then((fn) => fn());
      unlistenToken.then((fn) => fn());
      unlistenAnalysisComplete.then((fn) => fn());
      unlistenWorkflowState.then((fn) => fn());
      unlistenWorkflowComplete.then((fn) => fn());
      unlistenStartupWarning.then((fn) => fn());
    };
  }, []);

  // Auto-scroll
  useEffect(() => {
    if (resultRef.current && streamingResult) {
      resultRef.current.scrollTop = resultRef.current.scrollHeight;
    }
  }, [streamingResult]);

  // Error-Timeout: Fehlerbanner nach 15s automatisch ausblenden
  useEffect(() => {
    if (!audioError) return;
    const timer = setTimeout(() => setAudioError(""), 15000);
    return () => clearTimeout(timer);
  }, [audioError]);

  // Ollama-Status periodisch pruefen (alle 30s wenn error)
  useEffect(() => {
    if (ollamaStatus !== "error") return;
    const interval = setInterval(() => {
      console.log("[Diktat] Ollama-Reconnect-Versuch...");
      checkOllama();
    }, 30000);
    return () => clearInterval(interval);
  }, [ollamaStatus]);

  // --- Actions ---

  async function checkOllama() {
    try {
      const res = (await invoke("check_ollama_status")) as any;
      setOllamaStatus("ok");
      const models = (res?.models || []) as any[];
      setOllamaModels(models.map((m: any) => m.name));
      console.log("[Diktat] Ollama verbunden:", models.map((m: any) => m.name));
    } catch (e) {
      setOllamaStatus("error");
      console.warn("[Diktat] Ollama nicht erreichbar:", e);
    }
  }

  async function checkWhisper() {
    try {
      const res = (await invoke("check_whisper_model")) as any;
      if (res.valid) {
        setWhisperStatus("ok");
        console.log("[Diktat] Whisper-Modell gefunden:", res.path, `(${res.file_size_mb.toFixed(0)} MB)`);
      } else {
        setWhisperStatus("error");
        console.warn("[Diktat] Whisper-Modell fehlt:", res.path);
      }
    } catch (e) {
      setWhisperStatus("error");
      console.warn("[Diktat] Whisper-Check fehlgeschlagen:", e);
    }
  }

  async function handleAudioFile(filePath: string) {
    setAudioLoading(true);
    setAudioError("");
    setAudioInfo(null);
    setChunks([]);
    setResult("");
    setStreamingResult("");
    setTranscript("");
    setTranscriptChunks([]);
    setSelectedFile(filePath);
    setTranscriptionProgress(0);
    setCurrentChunkIndex(-1);
    setChunkStatuses({});
    setWorkflowPhase("Idle");

    try {
      const res = (await invoke("prepare_chunks", { path: filePath })) as any;
      setAudioInfo(res.audio_info as AudioInfo);
      setChunks(res.chunks as ChunkMeta[]);
      setTotalChunks(res.chunks.length);
      console.log(`[Diktat] Audio geladen: ${res.audio_info.filename}, ${res.chunks.length} Chunks`);
      const statuses: Record<number, string> = {};
      for (let i = 0; i < res.chunks.length; i++) {
        statuses[i] = "pending";
      }
      setChunkStatuses(statuses as any);
    } catch (e) {
      const msg = `Fehler beim Laden: ${e}`;
      setAudioError(msg);
      console.error("[Diktat] Audio-Fehler:", e);
    } finally {
      setAudioLoading(false);
    }
  }

  async function startTranscription() {
    if (!selectedFile) return;
    setTranscribing(true);
    setTranscriptionProgress(0);
    setCurrentChunkIndex(0);
    setTranscript("");
    setResult("");
    setStreamingResult("");
    setTranscriptChunks([]);
    setChunkStatuses((prev) => {
      const updated = { ...prev };
      if (Object.keys(updated).length > 0) updated[0] = "transcribing";
      return updated;
    });

    try {
      const res = (await invoke("transcribe_audio", {
        path: selectedFile,
        language,
      })) as TranscriptionResult;
      setTranscript(res.full_text);
      setTranscriptChunks(res.chunks);
      setTranscriptionProgress(100);
      console.log(`[Diktat] Transkription abgeschlossen: ${res.full_text.length} Zeichen, ${res.chunks.length} Chunks`);
    } catch (e) {
      const msg = `Transkription fehlgeschlagen: ${e}`;
      setAudioError(msg);
      console.error("[Diktat] Transkriptions-Fehler:", e);
      setTranscribing(false);
    }
  }

  async function openFileDialog() {
    const selected = await open({
      multiple: false,
      filters: [{ name: "Audio", extensions: ["wav", "mp3", "flac", "ogg", "m4a", "aac", "wma", "opus"] }],
    });
    if (selected) await handleAudioFile(selected as string);
  }

  const handleDrop = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setDragOver(false);
    const files = e.dataTransfer.getData("text/files") || e.dataTransfer.getData("text/plain");
    if (files) {
      try {
        const paths = JSON.parse(files);
        if (Array.isArray(paths) && paths.length > 0) handleAudioFile(paths[0]);
      } catch {
        if (files.trim()) handleAudioFile(files.trim());
      }
    }
  }, []);

  const handleDragOver = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setDragOver(true);
  }, []);

  const handleDragLeave = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setDragOver(false);
  }, []);

  async function runAnalysis(task: string) {
    if (!transcript.trim()) return;
    setLoading(true);
    setActiveTask(task);
    setResult("");
    setStreamingResult("");
    setActiveTab("analysis");

    try {
      await invoke("analyze_transcript_stream", { transcript, task });
      console.log(`[Diktat] Analyse abgeschlossen: ${task}`);
    } catch (e) {
      const msg = `Analyse-Fehler: ${e}`;
      setResult(msg);
      console.error("[Diktat] Analyse-Fehler:", e);
      setLoading(false);
      setActiveTask("");
    }
  }

  async function runWorkflow() {
    if (!selectedFile) return;
    setWorkflowRunning(true);
    setTranscribing(true);
    setLoading(true);
    setActiveTask(selectedWorkflowTask);
    setTranscriptionProgress(0);
    setCurrentChunkIndex(0);
    setTranscript("");
    setResult("");
    setStreamingResult("");
    setWorkflowPhase("LoadingAudio");
    setChunkStatuses((prev) => {
      const updated = { ...prev };
      if (Object.keys(updated).length > 0) updated[0] = "transcribing";
      return updated;
    });

    try {
      const res = (await invoke("run_workflow", {
        path: selectedFile,
        task: selectedWorkflowTask,
        language,
      })) as any;
      setTranscript(res.transcript);
      setResult(res.analysis);
      setStreamingResult(res.analysis);
      setTranscriptionProgress(100);
    } catch (e) {
      const msg = `Workflow fehlgeschlagen: ${e}`;
      setAudioError(msg);
      setWorkflowPhase("Error");
      console.error("[Diktat] Workflow-Fehler:", e);
    } finally {
      setWorkflowRunning(false);
      setTranscribing(false);
      setLoading(false);
      setActiveTask("");
    }
  }

  // --- Export ---

  async function handleExport(format: string) {
    const audioName = audioInfo?.filename || "audio";
    const baseName = audioName.replace(/\.[^.]+$/, "");

    const filters: { name: string; extensions: string[] }[] = [];
    let defaultExt = format;

    switch (format) {
      case "txt":
        filters.push({ name: "Text", extensions: ["txt"] });
        break;
      case "md":
        filters.push({ name: "Markdown", extensions: ["md"] });
        break;
      case "json":
        filters.push({ name: "JSON", extensions: ["json"] });
        break;
      case "srt":
        filters.push({ name: "Untertitel", extensions: ["srt"] });
        break;
    }

    const filePath = await save({
      defaultPath: `${baseName}.${defaultExt}`,
      filters,
    });

    if (!filePath) return;

    try {
      if (format === "srt") {
        if (transcriptChunks.length === 0) {
          setAudioError("SRT-Export braucht Chunk-Daten. Bitte zuerst transkribieren.");
          return;
        }
        const chunksData = JSON.stringify(
          transcriptChunks.map((c) => ({
            start_secs: c.start_secs,
            end_secs: c.end_secs,
            text: c.text,
          }))
        );
        await invoke("export_srt_file", { chunksJson: chunksData, path: filePath });
      } else {
        await invoke("export_result", {
          transcript,
          analysis: result || null,
          audioName,
          format,
          path: filePath,
        });
      }
    } catch (e) {
      const msg = `Export fehlgeschlagen: ${e}`;
      setAudioError(msg);
      console.error("[Diktat] Export-Fehler:", e);
    }
  }

  // --- Derived State ---

  const canAnalyze = transcript.trim().length > 0 && !loading && ollamaStatus === "ok";
  const canTranscribe = selectedFile.length > 0 && !transcribing && !workflowRunning && whisperStatus === "ok";
  const canRunWorkflow = selectedFile.length > 0 && !workflowRunning && !transcribing && !loading && ollamaStatus === "ok" && whisperStatus === "ok";
  const hasResult = !!(result || streamingResult);
  const isBusy = transcribing || loading || workflowRunning;

  // --- Render ---

  return (
    <div className="app-container">
      {/* Header */}
      <header className="app-header">
        <div>
          <h1>Diktat</h1>
          <p className="subtitle">Lokale Transkription &amp; Analyse</p>
        </div>
        <div className="header-actions">
          <button className="icon-btn" onClick={() => setShowSettings(!showSettings)} title="Einstellungen">
            &#9881;
          </button>
        </div>
      </header>

      {/* Startup Warnings */}
      {startupWarnings.length > 0 && (
        <div className="startup-warnings">
          {startupWarnings.map((w, i) => (
            <div key={i} className="warning-banner">{w}</div>
          ))}
          <button className="dismiss-btn" onClick={() => setStartupWarnings([])}>Schliessen</button>
        </div>
      )}

      {/* Status Bar */}
      <div className="status-bar">
        <span className={`status-item ${ollamaStatus}`}>
          <span className="status-dot" />
          Ollama:{" "}
          {ollamaStatus === "checking" ? "Pruefe..." : ollamaStatus === "ok" ? "Verbunden" : "Nicht erreichbar"}
        </span>
        <span className={`status-item ${whisperStatus}`}>
          <span className="status-dot" />
          Whisper:{" "}
          {whisperStatus === "checking" ? "Pruefe..." : whisperStatus === "ok" ? "Bereit" : "Modell fehlt"}
        </span>
        {ollamaModels.length > 0 && (
          <span className="status-item ok">
            {ollamaModels.join(", ")}
          </span>
        )}
        {audioInfo && (
          <span className="status-item neutral">
            {formatDuration(audioInfo.duration_secs)} | {chunks.length} Chunks
          </span>
        )}
      </div>

      {/* Settings Panel */}
      {showSettings && (
        <div className="settings-panel">
          <div className="settings-row">
            <label>Sprache</label>
            <select value={language} onChange={(e) => setLanguage(e.target.value)} className="settings-select">
              {LANGUAGES.map((l) => (
                <option key={l.code} value={l.code}>{l.label}</option>
              ))}
            </select>
          </div>
          <div className="settings-row">
            <label>Modell</label>
            <span className="settings-value">
              Whisper: large-v3-turbo | Analyse: {ollamaModels[0] || "n/a"}
            </span>
          </div>
        </div>
      )}

      {/* Audio Drop Zone */}
      <div
        className={`drop-zone ${dragOver ? "drag-over" : ""} ${audioLoading ? "loading" : ""}`}
        onDrop={handleDrop}
        onDragOver={handleDragOver}
        onDragLeave={handleDragLeave}
        onClick={openFileDialog}
      >
        {audioLoading ? (
          <p>Audio wird geladen und chunked...</p>
        ) : audioInfo ? (
          <div>
            <p style={{ fontWeight: "bold" }}>{audioInfo.filename}</p>
            <p>
              {formatDuration(audioInfo.duration_secs)} |{" "}
              {audioInfo.sample_rate / 1000} kHz | {audioInfo.channels} Kanal(e) | {formatFileSize(audioInfo.file_size)}
            </p>
            <p style={{ marginTop: "0.5rem", color: "#4a90d9" }}>
              {chunks.length} Chunks vorbereitet (30s Fenster)
            </p>
            <p style={{ fontSize: "0.75rem", marginTop: "0.5rem", color: "#888" }}>
              Klicken oder andere Datei ablegen zum Wechseln
            </p>
          </div>
        ) : (
          <>
            <p className="drop-zone-title">Audio-Datei hier ablegen oder klicken</p>
            <p className="drop-zone-hint">WAV, MP3, FLAC, OGG, M4A</p>
          </>
        )}
      </div>

      {audioError && (
        <div className="error-banner">{audioError}</div>
      )}

      {/* Workflow Section */}
      {chunks.length > 0 && !workflowRunning && workflowPhase !== "Complete" && !transcribing && (
        <div className="workflow-section">
          <h2>One-Click Workflow</h2>
          <p className="section-hint">Transkription + Analyse in einem Schritt</p>
          <div className="workflow-controls">
            <select
              className="task-select"
              value={selectedWorkflowTask}
              onChange={(e) => setSelectedWorkflowTask(e.target.value)}
            >
              {ANALYSIS_OPTIONS.map((opt) => (
                <option key={opt.key} value={opt.key}>
                  {opt.icon} {opt.label}
                </option>
              ))}
            </select>
            <button className="workflow-btn" disabled={!canRunWorkflow} onClick={runWorkflow}>
              Workflow starten
            </button>
          </div>
        </div>
      )}

      {/* Workflow Progress */}
      {workflowRunning && (
        <div className="workflow-progress">
          <div className="workflow-phases">
            <div className={`workflow-phase ${workflowPhase === "LoadingAudio" ? "active" : ["Transcribing", "Analyzing", "Complete"].includes(workflowPhase) ? "done" : ""}`}>
              <span className="phase-icon">&#9654;</span>
              <span>Audio laden</span>
            </div>
            <div className="workflow-arrow">&#8594;</div>
            <div className={`workflow-phase ${workflowPhase === "Transcribing" ? "active" : ["Analyzing", "Complete"].includes(workflowPhase) ? "done" : ""}`}>
              <span className="phase-icon">&#9654;</span>
              <span>Transkribieren</span>
              {workflowPhase === "Transcribing" && (
                <span className="phase-detail">Chunk {currentChunkIndex + 1}/{totalChunks}</span>
              )}
            </div>
            <div className="workflow-arrow">&#8594;</div>
            <div className={`workflow-phase ${workflowPhase === "Analyzing" ? "active" : workflowPhase === "Complete" ? "done" : ""}`}>
              <span className="phase-icon">&#9654;</span>
              <span>Analysieren</span>
              {workflowPhase === "Analyzing" && (
                <span className="phase-detail">{ANALYSIS_OPTIONS.find((o) => o.key === selectedWorkflowTask)?.label}</span>
              )}
            </div>
          </div>
          <div className="progress-bar" style={{ marginTop: "0.75rem" }}>
            <div className="progress-bar-fill" style={{ width: `${transcriptionProgress}%` }} />
          </div>
        </div>
      )}

      {/* Chunk List + Transcribe Button */}
      {chunks.length > 0 && !workflowRunning && (
        <details className="chunk-details">
          <summary className="chunk-summary">
            <span>Chunks ({chunks.length})</span>
            <button
              className="transcribe-btn"
              disabled={transcribing || !canTranscribe}
              onClick={(e) => { e.stopPropagation(); startTranscription(); }}
            >
              {transcribing ? "Transkribiert..." : "Transkribieren"}
            </button>
          </summary>

          {transcribing && (
            <div className="progress-section">
              <div className="progress-label">
                Chunk {currentChunkIndex + 1} / {totalChunks}
              </div>
              <div className="progress-bar">
                <div className="progress-bar-fill" style={{ width: `${transcriptionProgress}%` }} />
              </div>
            </div>
          )}

          <div className="chunk-list">
            {chunks.map((c) => (
              <div key={c.index} className="chunk-item">
                <span className="chunk-index">#{c.index + 1}</span>
                <span>{formatDuration(c.start_secs)} - {formatDuration(c.end_secs)}</span>
                <span className="chunk-samples">{(c.sample_count / 1000).toFixed(0)}k</span>
                <span className={`chunk-status ${chunkStatuses[c.index] || "pending"}`}>
                  {chunkStatuses[c.index] === "done" ? "OK" :
                   chunkStatuses[c.index] === "transcribing" ? "..." :
                   chunkStatuses[c.index] === "error" ? "!" : "-"}
                </span>
              </div>
            ))}
          </div>
        </details>
      )}

      {/* Results Area with Tabs */}
      {(transcript || hasResult) && (
        <div className="results-area">
          <div className="tab-bar">
            <button
              className={`tab-btn ${activeTab === "transcript" ? "active" : ""}`}
              onClick={() => setActiveTab("transcript")}
            >
              Transkript
              {transcript && <span className="tab-badge">{transcript.length > 1000 ? `${Math.round(transcript.length / 1000)}k` : transcript.length} Z</span>}
            </button>
            <button
              className={`tab-btn ${activeTab === "analysis" ? "active" : ""}`}
              onClick={() => setActiveTab("analysis")}
            >
              Analyse
              {activeTask && <span className="tab-badge active">läuft</span>}
            </button>
            <div className="tab-actions">
              {(transcript || hasResult) && (
                <div className="export-group">
                  <button className="export-btn" onClick={() => handleExport("txt")} disabled={isBusy} title="Als TXT exportieren">TXT</button>
                  <button className="export-btn" onClick={() => handleExport("md")} disabled={isBusy} title="Als Markdown exportieren">MD</button>
                  <button className="export-btn" onClick={() => handleExport("json")} disabled={isBusy} title="Als JSON exportieren">JSON</button>
                  {transcriptChunks.length > 0 && (
                    <button className="export-btn" onClick={() => handleExport("srt")} disabled={isBusy} title="Als SRT Untertitel exportieren">SRT</button>
                  )}
                </div>
              )}
            </div>
          </div>

          <div className="tab-content">
            {activeTab === "transcript" && (
              <textarea
                className="transcript-area"
                placeholder="Hier erscheint das Transkript nach der Spracherkennung. Du kannst auch manuell Text eingeben."
                value={transcript}
                onChange={(e) => setTranscript(e.target.value)}
              />
            )}

            {activeTab === "analysis" && (
              <>
                {loading && (
                  <div className="analysis-progress">
                    <div className="progress-label">
                      {ANALYSIS_OPTIONS.find((o) => o.key === activeTask)?.label || activeTask}
                    </div>
                    <div className="progress-bar">
                      <div className="progress-bar-fill analyzing" />
                    </div>
                  </div>
                )}

                <div className="actions">
                  {ANALYSIS_OPTIONS.map((opt) => (
                    <button
                      key={opt.key}
                      disabled={!canAnalyze}
                      onClick={() => runAnalysis(opt.key)}
                      className={`analysis-btn ${activeTask === opt.key ? "active" : ""}`}
                    >
                      <span className="btn-icon">{opt.icon}</span>
                      {opt.label}
                    </button>
                  ))}
                </div>

                <div className="result-box" ref={resultRef}>
                  {loading ? streamingResult || "Analysiere..." : result || "Waehle eine Analyseart oben."}
                </div>

                {result && (
                  <div className="result-footer">
                    <button className="copy-btn" onClick={() => navigator.clipboard.writeText(result)}>
                      Kopieren
                    </button>
                    <span className="result-meta">{result.length} Zeichen</span>
                  </div>
                )}
              </>
            )}
          </div>
        </div>
      )}

      {/* Empty State - show analysis section even without transcript */}
      {!transcript && !hasResult && !audioInfo && (
        <div className="empty-state">
          <h2>Transkript</h2>
          <textarea
            className="transcript-area"
            placeholder="Hier erscheint das Transkript nach der Spracherkennung. Du kannst auch manuell Text eingeben, um die Analyse zu testen."
            value={transcript}
            onChange={(e) => setTranscript(e.target.value)}
          />

          {transcript.trim().length > 0 && (
            <>
              <div className="actions">
                {ANALYSIS_OPTIONS.map((opt) => (
                  <button
                    key={opt.key}
                    disabled={!canAnalyze}
                    onClick={() => runAnalysis(opt.key)}
                    className={`analysis-btn ${activeTask === opt.key ? "active" : ""}`}
                  >
                    <span className="btn-icon">{opt.icon}</span>
                    {opt.label}
                  </button>
                ))}
              </div>

              {hasResult && (
                <div className="result-box" ref={resultRef}>
                  {loading ? streamingResult || "Analysiere..." : result}
                </div>
              )}
            </>
          )}
        </div>
      )}
    </div>
  );
}

export default App;
