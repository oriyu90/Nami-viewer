/**
 * NamiVision mock backend for plain-browser development and UI QA.
 *
 * The real application talks to Rust through Tauri's `invoke` bridge. When
 * that bridge is absent `app.js` dynamically imports this module instead, so
 * the complete UI can be exercised without a native window. The synthetic
 * track is a 12 second, 120 BPM click pattern labelled
 * `demo_click_120bpm.wav`; playback is simulated (no audio device).
 */

import {
  BAND_LABELS,
  BAND_PALETTE,
  BEAT_TOLERANCE,
  buildBeatCsv,
  clamp,
  defaultSettings,
  normalizeBands,
  normalizeProject,
  normalizeSettings,
  sanitizeCsvFilename,
} from '../js/core.js';

const AUDIO_RE = /\.(mp3|wav|flac|ogg|oga)$/i;
const STORE = {
  settings: 'namivision.mock.settings',
  project: 'namivision.mock.project',
  projectPath: 'namivision.mock.projectPath',
  lastAudio: 'namivision.mock.lastAudio',
};

const DEMO_PATH = 'C:\\Music\\demo_click_120bpm.wav';
const DEMO_PROJECT_PATH = 'C:\\Music\\NamiVision\\demo_project.nvproj';
const DEMO_MISSING_PROJECT_PATH = 'C:\\Music\\NamiVision\\demo_missing_audio.nvproj';
const SAMPLE_RATE = 44100;
const HOP_SEC = 512 / SAMPLE_RATE;
const BEAT_SEC = 0.5;

const listeners = new Map();
const dragListeners = new Set();

let lastPath = null;
let lastAnalysis = null;

const playback = {
  positionSec: 0,
  durationSec: 12,
  playing: false,
  volume: 0.8,
  ended: false,
  basePos: 0,
  startedAt: 0,
};

/* ------------------------------------------------------------------ */
/* storage / helpers                                                   */
/* ------------------------------------------------------------------ */

function readJson(key) {
  try {
    const raw = localStorage.getItem(key);
    return raw ? JSON.parse(raw) : null;
  } catch {
    return null;
  }
}

function writeJson(key, value) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* private mode / quota — mock persistence is best effort */
  }
}

function appError(code, params = {}, message = '') {
  return { code, message, params };
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function emit(event, payload) {
  const set = listeners.get(event);
  if (!set) return;
  for (const handler of [...set]) {
    try {
      handler({ event, payload });
    } catch {
      /* a listener must not break the mock backend */
    }
  }
}

function basename(path) {
  const parts = String(path || '').split(/[\\/]/);
  return parts[parts.length - 1] || '';
}

function extensionOf(path) {
  const match = /\.([A-Za-z0-9]+)$/.exec(String(path || ''));
  return match ? match[1].toLowerCase() : '';
}

function storedSettings() {
  const stored = readJson(STORE.settings);
  return normalizeSettings(stored || defaultSettings());
}

function settingsWithUi() {
  const settings = storedSettings();
  const ui = readJson('namivision.ui');
  if (ui && typeof ui === 'object') {
    if (ui.lang === 'ja' || ui.lang === 'en') settings.lang = ui.lang;
    if (ui.theme === 'dark' || ui.theme === 'light') settings.theme = ui.theme;
    if (Number.isFinite(Number(ui.scale))) settings.scale = normalizeSettings({ ...settings, scale: ui.scale }).scale;
  }
  return settings;
}

function audioDurationFor(path) {
  if (/short/i.test(path)) return 3.2;
  if (/long/i.test(path)) return 1300;
  return 12;
}

function validateAudioPath(path) {
  if (!AUDIO_RE.test(path)) {
    throw appError('ERR_UNSUPPORTED_FORMAT', { format: extensionOf(path) || 'unknown' });
  }
}

/* ------------------------------------------------------------------ */
/* synthetic analysis                                                  */
/* ------------------------------------------------------------------ */

function percentile(values, ratio) {
  if (!values.length) return 0;
  const sorted = [...values].sort((a, b) => a - b);
  const index = clamp(Math.floor((sorted.length - 1) * ratio), 0, sorted.length - 1);
  return sorted[index];
}

function makeEvents(duration) {
  const events = [];
  for (let beat = 0; beat * BEAT_SEC < duration; beat += 1) {
    const t = beat * BEAT_SEC;
    if (beat === 7) continue; // leave one silent beat so undetected CSV rows exist
    events.push({ t: t + 0.015, freq: 60, amp: 0.95, decay: 0.2, kind: 'kick' });
    events.push({ t: t + 0.004, freq: 1200, amp: 0.55, decay: 0.12, kind: 'snare' });
    events.push({ t: t - 0.008, freq: 8000, amp: 0.72, decay: 0.05, kind: 'hat' });
    if (t + 0.25 < duration) {
      events.push({ t: t + 0.25, freq: 8000, amp: 0.28, decay: 0.035, kind: 'hat' });
    }
  }
  return events.sort((a, b) => a.t - b.t);
}

function spectralWeight(lowHz, highHz, freq) {
  const low = Math.max(1, Number(lowHz) || 20);
  const high = Math.max(low, Number(highHz) || 20000);
  const center = Math.sqrt(low * high);
  const octaves = Math.log2(Math.max(1, freq) / center);
  return Math.exp(-(octaves * octaves) / (2 * 1.15 * 1.15));
}

function waveformAt(time, events) {
  let value = 0;
  for (const event of events) {
    const dt = time - event.t;
    if (dt < 0 || dt > 0.6) continue;
    const amp = event.amp * Math.exp(-dt / event.decay);
    if (event.kind === 'kick') {
      value += Math.sin(2 * Math.PI * 60 * dt) * amp;
    } else if (event.kind === 'snare') {
      value += (
        Math.sin(2 * Math.PI * 180 * dt) * 0.7
        + Math.sin(2 * Math.PI * 1200 * dt) * 0.5
        + Math.sin(2 * Math.PI * 3000 * dt) * 0.2
      ) * amp;
    } else {
      value += (
        Math.sin(2 * Math.PI * 8000 * dt)
        + Math.sin(2 * Math.PI * 13000 * dt) * 0.4
      ) * amp;
    }
  }
  return clamp(value, -1, 1);
}

function makePeaks(duration, events) {
  const count = Math.max(1, Math.ceil(duration * 500));
  const min = new Array(count);
  const max = new Array(count);
  const step = 1 / 500;
  for (let i = 0; i < count; i += 1) {
    const t0 = i * step;
    let lo = 0;
    let hi = 0;
    for (let s = 0; s < 4; s += 1) {
      const t = t0 + ((s + 0.5) * step) / 4;
      const value = waveformAt(t, events);
      if (value < lo) lo = value;
      if (value > hi) hi = value;
    }
    min[i] = lo;
    max[i] = hi;
  }
  return { min, max };
}

function makeBandData(lowHz, highHz, frameCount, events) {
  const raw = new Array(frameCount);
  for (let f = 0; f < frameCount; f += 1) {
    const t = f * HOP_SEC;
    let energy = 0;
    for (const event of events) {
      const dt = t - event.t;
      if (dt < 0 || dt > 0.7) continue;
      const weight = spectralWeight(lowHz, highHz, event.freq);
      if (weight < 0.0005) continue;
      energy += event.amp * weight * Math.exp(-dt / event.decay);
    }
    raw[f] = energy;
  }

  let peak = 0;
  for (const value of raw) if (value > peak) peak = value;
  const envelope = raw.map((value) => {
    const normalized = peak > 0 ? clamp(value / peak, 0, 1) : 0;
    return normalized ** 0.75;
  });

  const diff = new Array(frameCount);
  for (let f = 0; f < frameCount; f += 1) {
    if (f === 0) {
      diff[f] = 0;
      continue;
    }
    diff[f] = Math.max(0, Math.log(raw[f] + 1e-9) - Math.log(raw[f - 1] + 1e-9));
  }
  const smooth = new Array(frameCount);
  for (let f = 0; f < frameCount; f += 1) {
    const a = diff[Math.max(0, f - 1)];
    const b = diff[f];
    const c = diff[Math.min(frameCount - 1, f + 1)];
    smooth[f] = (a + b + c) / 3;
  }
  const onsetPeak = percentile(smooth, 0.99) || 0;
  const onset = smooth.map((value) => (onsetPeak > 0 ? clamp(value / onsetPeak, 0, 1) : 0));

  return { envelope, onset };
}

function buildBands(duration, bandDefs) {
  const frameCount = Math.max(1, Math.floor(duration / HOP_SEC) + 1);
  const events = makeEvents(duration);
  return bandDefs.map((band, index) => {
    const lowHz = Number(band.lowHz);
    const highHz = Number(band.highHz);
    const data = makeBandData(lowHz, highHz, frameCount, events);
    return {
      index,
      label: BAND_LABELS[index] || `Band ${index + 1}`,
      lowHz,
      highHz,
      color: band.color || BAND_PALETTE[index] || BAND_PALETTE[0],
      envelope: data.envelope,
      onset: data.onset,
    };
  });
}

function buildAnalysis(path, duration, bands) {
  const format = extensionOf(path) || 'wav';
  const fileName = basename(path) || 'demo_click_120bpm.wav';
  const events = makeEvents(duration);
  const peaks = makePeaks(duration, events);
  const frameCount = Math.max(1, Math.floor(duration / HOP_SEC) + 1);
  const edges = new Array(97);
  for (let j = 0; j <= 96; j += 1) edges[j] = 20 * (1000 ** (j / 96));

  return {
    meta: {
      path,
      fileName,
      format,
      sampleRate: SAMPLE_RATE,
      channels: 2,
      durationSec: duration,
      bitDepth: 16,
    },
    peakBucketsPerSecond: 500,
    peaksMin: peaks.min,
    peaksMax: peaks.max,
    frameHopSec: HOP_SEC,
    frameCount,
    logBandCount: 96,
    logBandEdgesHz: edges,
    bands: buildBands(duration, bands),
    analysisMs: 486,
    warning: duration > 1200 ? 'longAudio' : null,
  };
}

async function analysisWithProgress(path, duration, bands) {
  const phases = [
    { phase: 'decode', progress: 0.18, wait: 90 },
    { phase: 'decode', progress: 0.34, wait: 80 },
    { phase: 'fft', progress: 0.56, wait: 110 },
    { phase: 'fft', progress: 0.78, wait: 100 },
    { phase: 'bands', progress: 0.92, wait: 90 },
    { phase: 'bands', progress: 1, wait: 60 },
  ];
  for (const step of phases) {
    emit('analysis-progress', { phase: step.phase, progress: step.progress });
    await sleep(step.wait);
  }
  return buildAnalysis(path, duration, bands);
}

/* ------------------------------------------------------------------ */
/* project / settings                                                  */
/* ------------------------------------------------------------------ */

function demoMissingProject() {
  const settings = settingsWithUi();
  return {
    schema: 'namivision.project',
    version: 1,
    appVersion: '0.1.0',
    savedAt: new Date().toISOString(),
    audio: {
      path: 'C:\\Music\\missing_demo.wav',
      fileName: 'missing_demo.wav',
      durationSec: 12,
    },
    tempo: {
      enabled: true,
      bpm: 120,
      beatsPerBar: 4,
      barStartSec: 0.25,
      subdivision: 2,
      showGrid: true,
      snapToGrid: false,
    },
    view: {
      zoomPxPerSec: 96,
      scrollSec: 0,
      selectedBand: 1,
      followPlayhead: true,
    },
    bands: settings.bands,
    playback: { positionSec: 0, volume: settings.volume },
    ui: { lang: settings.lang, theme: settings.theme, scale: settings.scale },
  };
}

function currentDuration() {
  return lastAnalysis?.meta?.durationSec || playback.durationSec || 12;
}

function syncPlaybackPosition() {
  if (!playback.playing) return;
  const duration = currentDuration();
  const position = playback.basePos + (performance.now() - playback.startedAt) / 1000;
  if (position >= duration) {
    playback.positionSec = duration;
    playback.playing = false;
    playback.ended = true;
  } else {
    playback.positionSec = Math.max(0, position);
  }
}

function setPlaybackPosition(seconds) {
  const duration = currentDuration();
  playback.positionSec = clamp(Number(seconds) || 0, 0, duration);
  playback.ended = false;
  if (playback.playing) {
    playback.basePos = playback.positionSec;
    playback.startedAt = performance.now();
  }
}

function downloadCsv(filename, csv) {
  if (typeof document === 'undefined' || typeof URL === 'undefined') return;
  try {
    const blob = new Blob([csv], { type: 'text/csv;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = filename;
    anchor.style.display = 'none';
    document.body.appendChild(anchor);
    anchor.click();
    anchor.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1500);
  } catch {
    /* download is best effort in the mock */
  }
}

/* ------------------------------------------------------------------ */
/* public API                                                          */
/* ------------------------------------------------------------------ */

export function listen(event, handler) {
  if (typeof handler !== 'function') return () => {};
  if (!listeners.has(event)) listeners.set(event, new Set());
  listeners.get(event).add(handler);
  return () => {
    const set = listeners.get(event);
    if (set) set.delete(handler);
  };
}

export function onDragDropEvent(handler) {
  if (typeof handler !== 'function') return () => {};
  dragListeners.add(handler);
  return () => dragListeners.delete(handler);
}

export async function invoke(command, args = {}) {
  switch (command) {
    case 'app_info':
      return {
        version: '0.1.0',
        dataDir: 'C:\\Users\\Demo\\AppData\\Roaming\\com.namivision.app',
        portable: false,
        webview2: typeof navigator !== 'undefined' ? navigator.userAgent : 'mock',
      };

    case 'startup_args':
      return { openPath: null, projectPath: null };

    case 'pick_audio':
      return { path: DEMO_PATH };

    case 'load_audio': {
      const path = String(args.path || DEMO_PATH);
      validateAudioPath(path);
      const duration = audioDurationFor(path);
      if (duration < 5) {
        throw appError('ERR_AUDIO_TOO_SHORT', { min: '5.0', actual: duration.toFixed(1) });
      }
      const settings = settingsWithUi();
      lastPath = path;
      writeJson(STORE.lastAudio, path);
      lastAnalysis = await analysisWithProgress(path, duration, settings.bands);
      playback.durationSec = duration;
      setPlaybackPosition(0);
      playback.playing = false;
      playback.ended = false;
      playback.volume = settings.volume;
      return lastAnalysis;
    }

    case 'reanalyze': {
      const path = String(args.path || lastPath || DEMO_PATH);
      validateAudioPath(path);
      const settings = settingsWithUi();
      lastPath = path;
      lastAnalysis = await analysisWithProgress(path, audioDurationFor(path), settings.bands);
      playback.durationSec = lastAnalysis.meta.durationSec;
      setPlaybackPosition(playback.positionSec);
      return lastAnalysis;
    }

    case 'set_bands': {
      const source = Array.isArray(args.bands) ? args.bands : storedSettings().bands;
      const bands = normalizeBands(source, source.length);
      const duration = currentDuration();
      const data = buildBands(duration, bands);
      if (lastAnalysis) {
        lastAnalysis = { ...lastAnalysis, bands: data };
      }
      return { bands: data };
    }

    case 'pick_project':
      return {
        path: readJson(STORE.project) ? (readJson(STORE.projectPath) || DEMO_PROJECT_PATH) : DEMO_MISSING_PROJECT_PATH,
      };

    case 'load_project': {
      const stored = readJson(STORE.project);
      if (stored) {
        return { project: normalizeProject(stored), audioMissing: false };
      }
      return { project: normalizeProject(demoMissingProject()), audioMissing: true };
    }

    case 'save_project': {
      const raw = args.project && typeof args.project === 'object' ? args.project : {};
      const project = {
        ...raw,
        schema: 'namivision.project',
        version: 1,
        appVersion: '0.1.0',
        savedAt: new Date().toISOString(),
      };
      writeJson(STORE.project, project);
      const stem = String(project.audio?.fileName || 'project').replace(/\.[^.]+$/, '');
      const safe = stem.replace(/[^A-Za-z0-9_-]+/g, '_').replace(/^_+|_+$/g, '') || 'project';
      const path = `C:\\Music\\NamiVision\\${safe}.nvproj`;
      writeJson(STORE.projectPath, path);
      return { path };
    }

    case 'load_settings':
      return settingsWithUi();

    case 'save_settings': {
      const settings = normalizeSettings(args.settings || defaultSettings());
      writeJson(STORE.settings, settings);
      playback.volume = settings.volume;
      return { ok: true };
    }

    case 'export_beats_csv':
    case 'export_beats_csv_to': {
      if (!lastAnalysis) {
        throw appError('ERR_NO_AUDIO', {}, 'No audio loaded');
      }
      const tempo = args.tempo || {};
      if (!tempo.enabled) {
        throw appError('ERR_TEMPO_REQUIRED', {}, 'Tempo is not set');
      }
      const bandIndex = Math.round(clamp(Number(args.bandIndex) || 0, 0, Math.max(0, lastAnalysis.bands.length - 1)));
      const band = lastAnalysis.bands[bandIndex];
      const result = buildBeatCsv({
        analysis: lastAnalysis,
        bandIndex,
        tempo,
        sensitivity: storedSettings().onsetSensitivity,
        tolerance: Number.isFinite(Number(args.tolerance)) ? Number(args.tolerance) : BEAT_TOLERANCE,
        includeUndetected: args.includeUndetected !== false,
        delimiter: args.delimiter === ';' ? ';' : ',',
        decimals: Number.isFinite(Number(args.decimals)) ? Number(args.decimals) : 3,
      });
      const stem = String(lastAnalysis.meta?.fileName || 'audio').replace(/\.[^.]+$/, '');
      const filename = sanitizeCsvFilename(stem, bandIndex, band?.label || 'band');
      const path = args.path || `C:\\Users\\Demo\\Downloads\\${filename}`;
      if (command === 'export_beats_csv') downloadCsv(filename, result.csv);
      return { path, rows: result.rows, detected: result.detected, canceled: false };
    }

    case 'play':
      if (playback.ended) setPlaybackPosition(0);
      playback.playing = true;
      playback.ended = false;
      playback.basePos = playback.positionSec;
      playback.startedAt = performance.now();
      return { ok: true };

    case 'pause':
      syncPlaybackPosition();
      playback.playing = false;
      return { ok: true };

    case 'stop':
      playback.playing = false;
      playback.ended = false;
      playback.positionSec = 0;
      playback.basePos = 0;
      return { ok: true };

    case 'seek':
      setPlaybackPosition(args.seconds);
      return { ok: true };

    case 'set_volume':
      playback.volume = clamp(Number(args.volume) || 0, 0, 1);
      return { ok: true };

    case 'playback_state':
      syncPlaybackPosition();
      return {
        positionSec: playback.positionSec,
        durationSec: currentDuration(),
        playing: playback.playing,
        volume: playback.volume,
        ended: playback.ended,
      };

    case 'open_path':
    case 'reveal_file':
      return { ok: true };

    case 'quit':
      return { ok: true };

    default:
      throw appError('ERR_UNKNOWN', {}, `Unknown mock command: ${command}`);
  }
}
