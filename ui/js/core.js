/**
 * NamiVision — pure core helpers.
 *
 * DOM-free by design: the browser UI, the mock backend and `node --test`
 * all import this module. Keep every function deterministic and total
 * (never throw on malformed input, never return NaN).
 */

export const ZOOM_MIN = 2;
export const ZOOM_MAX = 4000;
export const DEFAULT_ZOOM = 96;
export const MIN_AUDIO_SEC = 5;
export const MAX_AUDIO_SEC = 3600;
export const LONG_AUDIO_SEC = 1200;
export const BEAT_TOLERANCE = 0.45;

export const BAND_PALETTE = ['#4FC3F7', '#7BD88F', '#FFC857', '#FF7A8A'];
export const BAND_LABELS = ['Low', 'Low-Mid', 'Mid-High', 'High'];
export const DEFAULT_CROSSOVERS = [250, 1000, 4000];
export const BAND_MIN_HZ = 20;
export const BAND_MAX_HZ = 20000;
export const BAND_MIN_WIDTH_HZ = 10;
export const BAND_TOP_HZ = 20000;

export const TICK_STEPS = [
  0.1, 0.2, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600,
];

export const METERS = [2, 3, 4, 6, 8];
export const SUBDIVISIONS = [1, 2, 4];
export const FFT_SIZES = [1024, 2048, 4096];

/* ------------------------------------------------------------------ */
/* coercion                                                            */
/* ------------------------------------------------------------------ */

export function clamp(value, min, max) {
  const n = Number(value);
  if (!Number.isFinite(n)) return min;
  return Math.min(max, Math.max(min, n));
}

export function asNumber(value, fallback) {
  const n = typeof value === 'string' && value.trim() !== '' ? Number(value) : value;
  return Number.isFinite(n) ? n : fallback;
}

export function asInt(value, fallback) {
  const n = asNumber(value, fallback);
  return Number.isFinite(n) ? Math.trunc(n) : fallback;
}

export function asBool(value, fallback) {
  return typeof value === 'boolean' ? value : fallback;
}

export function asString(value, fallback = '') {
  return typeof value === 'string' ? value : fallback;
}

export function euclideanMod(n, m) {
  if (!Number.isFinite(n) || !Number.isFinite(m) || m === 0) return 0;
  return ((n % m) + m) % m;
}

export function basename(path) {
  if (typeof path !== 'string' || path.length === 0) return '';
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || '';
}

/* ------------------------------------------------------------------ */
/* time formatting                                                     */
/* ------------------------------------------------------------------ */

/**
 * Format seconds as mm:ss.cc (or h:mm:ss.cc past one hour).
 * @param {number} seconds
 * @param {{decimals?: 0|1|2|3, forceHours?: boolean}} [opts]
 */
export function formatTime(seconds, opts = {}) {
  const decimals = Math.round(clamp(opts.decimals == null ? 2 : opts.decimals, 0, 3));
  const safe = Number.isFinite(seconds) ? Math.max(0, seconds) : 0;
  const unit = 10 ** decimals;
  let units = Math.round(safe * unit);
  const perHour = 3600 * unit;
  const perMinute = 60 * unit;
  const hours = Math.floor(units / perHour);
  units -= hours * perHour;
  const minutes = Math.floor(units / perMinute);
  units -= minutes * perMinute;
  const secs = Math.floor(units / unit);
  const frac = units - secs * unit;
  const pad = (v, width = 2) => String(v).padStart(width, '0');
  const fracText = decimals > 0 ? `.${pad(frac, decimals)}` : '';
  if (hours > 0 || opts.forceHours) {
    return `${hours}:${pad(minutes)}:${pad(secs)}${fracText}`;
  }
  return `${pad(minutes)}:${pad(secs)}${fracText}`;
}

export function formatSampleRate(sampleRate) {
  const sr = asNumber(sampleRate, 0);
  if (sr <= 0) return '—';
  if (sr >= 1000) {
    const khz = sr / 1000;
    const text = Number.isInteger(khz) ? String(khz) : khz.toFixed(2).replace(/0+$/, '').replace(/\.$/, '');
    return `${text} kHz`;
  }
  return `${Math.round(sr)} Hz`;
}

export function formatAnalysisMs(ms) {
  const value = asNumber(ms, -1);
  if (value < 0) return '—';
  if (value < 1000) return `${Math.round(value)} ms`;
  return `${(value / 1000).toFixed(2).replace(/0+$/, '').replace(/\.$/, '')} s`;
}

export function formatHzRange(lowHz, highHz) {
  const low = asNumber(lowHz, 0);
  const high = asNumber(highHz, 0);
  const trim = (v) => {
    const rounded = Math.round(v * 100) / 100;
    return Number.isInteger(rounded) ? String(rounded) : String(rounded).replace(/0+$/, '').replace(/\.$/, '');
  };
  if (high >= 1000) {
    return `${trim(low / 1000)}–${trim(high / 1000)} kHz`;
  }
  return `${trim(low)}–${trim(high)} Hz`;
}

export function formatBpm(bpm) {
  const n = asNumber(bpm, 0);
  return Number.isInteger(n) ? String(n) : n.toFixed(1).replace(/\.0$/, '');
}

/* ------------------------------------------------------------------ */
/* adaptive ruler                                                      */
/* ------------------------------------------------------------------ */

export function chooseTickStep(pxPerSec, minPx = 64) {
  const px = Number.isFinite(pxPerSec) && pxPerSec > 0 ? pxPerSec : DEFAULT_ZOOM;
  const spacing = Number.isFinite(minPx) && minPx > 0 ? minPx : 64;
  for (const step of TICK_STEPS) {
    if (step * px >= spacing) return step;
  }
  return TICK_STEPS[TICK_STEPS.length - 1];
}

export function generateTicks(startSec, endSec, stepSec) {
  const out = [];
  const step = asNumber(stepSec, 0);
  const start = asNumber(startSec, 0);
  const end = asNumber(endSec, 0);
  if (!(step > 0) || !(end >= start)) return out;
  const first = Math.ceil((start - 1e-9) / step) * step;
  for (let t = first; t <= end + 1e-9 && out.length < 20000; t += step) {
    const raw = Math.round(t * 1e6) / 1e6;
    const rounded = raw === 0 ? 0 : raw;
    if (rounded >= -1e-9) out.push(rounded);
  }
  return out;
}

/* ------------------------------------------------------------------ */
/* beat grid                                                           */
/* ------------------------------------------------------------------ */

export function beatDuration(bpm) {
  return 60 / clamp(bpm, 20, 400);
}

export function barDuration(tempo) {
  const bpm = clamp(tempo?.bpm, 20, 400);
  const meter = METERS.includes(Number(tempo?.beatsPerBar)) ? Number(tempo.beatsPerBar) : 4;
  return (60 / bpm) * meter;
}

export function beatTime(tempo, k) {
  const beatDur = beatDuration(tempo?.bpm);
  return asNumber(tempo?.barStartSec, 0) + k * beatDur;
}

export function barNumberForBeat(k, beatsPerBar) {
  const meter = METERS.includes(Number(beatsPerBar)) ? Number(beatsPerBar) : 4;
  return Math.floor(k / meter) + 1;
}

export function beatInBar(k, beatsPerBar) {
  const meter = METERS.includes(Number(beatsPerBar)) ? Number(beatsPerBar) : 4;
  return euclideanMod(k, meter) + 1;
}

/**
 * Generate visible beat / bar / subdivision lines.
 * Negative beat indices are produced when `barStartSec` leaves beats before
 * the bar start inside the audio (bar numbers follow the Euclidean rule).
 */
export function generateGrid(tempo, durationSec, viewStart = 0, viewEnd = durationSec, opts = {}) {
  const t = normalizeTempo(tempo);
  const duration = Math.max(0, asNumber(durationSec, 0));
  const beatDur = beatDuration(t.bpm);
  const barDur = beatDur * t.beatsPerBar;
  const result = { beats: [], bars: [], subdivisions: [], beatDur, barDur };
  if (!t.enabled || duration <= 0) return result;

  const maxBeats = Math.max(100, asInt(opts.maxBeats, 20000));
  const rawStart = asNumber(viewStart, 0);
  const rawEnd = asNumber(viewEnd, duration);
  const start = clamp(Math.min(rawStart, rawEnd), 0, duration);
  const end = clamp(Math.max(rawStart, rawEnd), 0, duration);
  const barStart = clamp(t.barStartSec, 0, duration);
  const kMin = Math.floor((start - barStart) / beatDur + 1e-9);
  const kMax = Math.ceil((end - barStart) / beatDur - 1e-9);
  const maxSubs = maxBeats * Math.max(1, t.subdivision);

  for (let k = kMin; k <= kMax; k += 1) {
    const time = barStart + k * beatDur;
    if (time < -1e-9 || time > duration + 1e-9) continue;
    if (time < start - 1e-9 || time > end + 1e-9) continue;
    const isBar = euclideanMod(k, t.beatsPerBar) === 0;
    const beat = {
      k,
      time,
      bar: barNumberForBeat(k, t.beatsPerBar),
      beatInBar: beatInBar(k, t.beatsPerBar),
      isBar,
    };
    if (result.beats.length < maxBeats) result.beats.push(beat);
    if (isBar && result.bars.length < maxBeats) result.bars.push(beat);
    if (t.subdivision > 1 && result.subdivisions.length < maxSubs) {
      for (let j = 1; j < t.subdivision; j += 1) {
        const subTime = time + (j * beatDur) / t.subdivision;
        if (subTime >= start - 1e-9 && subTime <= end + 1e-9 && subTime <= duration + 1e-9) {
          result.subdivisions.push(subTime);
        }
      }
    }
  }
  return result;
}

/* ------------------------------------------------------------------ */
/* view transforms                                                     */
/* ------------------------------------------------------------------ */

export function timeToX(time, view) {
  const zoom = asNumber(view?.zoomPxPerSec, DEFAULT_ZOOM);
  const scroll = asNumber(view?.scrollSec, 0);
  const t = asNumber(time, 0);
  return (t - scroll) * zoom;
}

export function xToTime(x, view) {
  const zoom = asNumber(view?.zoomPxPerSec, DEFAULT_ZOOM);
  const scroll = asNumber(view?.scrollSec, 0);
  const px = asNumber(x, 0);
  if (!(zoom > 0)) return scroll;
  return scroll + px / zoom;
}

export function clampView(view, durationSec, viewportWidth) {
  const duration = Math.max(0, asNumber(durationSec, 0));
  const width = Math.max(0, asNumber(viewportWidth, 0));
  const zoom = clamp(asNumber(view?.zoomPxPerSec, DEFAULT_ZOOM), ZOOM_MIN, ZOOM_MAX);
  const maxScroll = Math.max(0, duration - width / zoom);
  const scroll = clamp(asNumber(view?.scrollSec, 0), 0, maxScroll);
  const selectedBand = Math.round(clamp(asNumber(view?.selectedBand, 0), 0, 3));
  return {
    ...view,
    zoomPxPerSec: zoom,
    scrollSec: scroll,
    selectedBand,
    followPlayhead: asBool(view?.followPlayhead, true),
  };
}

export function zoomViewAt(view, anchorSec, newZoom, durationSec, viewportWidth) {
  const current = clampView(view, durationSec, viewportWidth);
  const anchor = asNumber(anchorSec, current.scrollSec);
  const oldZoom = current.zoomPxPerSec;
  const zoom = clamp(asNumber(newZoom, oldZoom), ZOOM_MIN, ZOOM_MAX);
  const anchorX = (anchor - current.scrollSec) * oldZoom;
  const scroll = anchor - anchorX / zoom;
  return clampView(
    { ...current, zoomPxPerSec: zoom, scrollSec: scroll },
    durationSec,
    viewportWidth,
  );
}

export function fitView(durationSec, viewportWidth, opts = {}) {
  const duration = Math.max(0, asNumber(durationSec, 0));
  const width = Math.max(0, asNumber(viewportWidth, 0));
  const padding = Math.max(0, asNumber(opts.padding, 16));
  const usable = Math.max(1, width - padding);
  const zoom = duration > 0 ? clamp(usable / duration, ZOOM_MIN, ZOOM_MAX) : DEFAULT_ZOOM;
  return clampView(
    { zoomPxPerSec: zoom, scrollSec: 0, selectedBand: opts.selectedBand ?? 0, followPlayhead: true },
    duration,
    width,
  );
}

export function visibleRange(view, viewportWidth) {
  const zoom = Math.max(ZOOM_MIN, asNumber(view?.zoomPxPerSec, DEFAULT_ZOOM));
  const scroll = Math.max(0, asNumber(view?.scrollSec, 0));
  const width = Math.max(0, asNumber(viewportWidth, 0));
  return { startSec: scroll, endSec: scroll + width / zoom };
}

/* ------------------------------------------------------------------ */
/* filenames                                                           */
/* ------------------------------------------------------------------ */

export function sanitizeFilenamePart(value, fallback = 'audio') {
  const text = String(value == null ? '' : value)
    .replace(/[^A-Za-z0-9_-]+/g, '_')
    .replace(/_+/g, '_')
    .replace(/^_+|_+$/g, '');
  return text || fallback;
}

export function sanitizeCsvFilename(stem, bandIndex, label) {
  const safeStem = sanitizeFilenamePart(stem);
  const safeLabel = sanitizeFilenamePart(label || 'band', 'band');
  const index = Math.max(0, asInt(bandIndex, 0));
  return `${safeStem}_band${index + 1}_${safeLabel}_beats.csv`;
}

/* ------------------------------------------------------------------ */
/* bands                                                               */
/* ------------------------------------------------------------------ */

export function defaultBands() {
  return [
    { lowHz: 20, highHz: 250, color: BAND_PALETTE[0] },
    { lowHz: 250, highHz: 1000, color: BAND_PALETTE[1] },
    { lowHz: 1000, highHz: 4000, color: BAND_PALETTE[2] },
    { lowHz: 4000, highHz: 16000, color: BAND_PALETTE[3] },
  ];
}

export function normalizeBand(raw, index = 0) {
  const fallback = defaultBands()[clamp(index, 0, 3)] || defaultBands()[0];
  let low = clamp(asNumber(raw?.lowHz, fallback.lowHz), BAND_MIN_HZ, BAND_MAX_HZ - BAND_MIN_WIDTH_HZ);
  let high = clamp(asNumber(raw?.highHz, fallback.highHz), BAND_MIN_HZ, BAND_MAX_HZ);
  if (high - low < BAND_MIN_WIDTH_HZ) {
    high = Math.min(BAND_MAX_HZ, low + BAND_MIN_WIDTH_HZ);
    if (high - low < BAND_MIN_WIDTH_HZ) low = Math.max(BAND_MIN_HZ, high - BAND_MIN_WIDTH_HZ);
  }
  const color = typeof raw?.color === 'string' && /^#[0-9a-fA-F]{6}$/.test(raw.color)
    ? raw.color.toUpperCase()
    : BAND_PALETTE[clamp(index, 0, 3)];
  return { lowHz: low, highHz: high, color };
}

export function normalizeBands(raw, count = 4) {
  const n = Math.round(clamp(asNumber(count, 4), 1, 4));
  const source = Array.isArray(raw) ? raw : [];
  const out = [];
  for (let i = 0; i < n; i += 1) {
    out.push(normalizeBand(source[i], i));
  }
  return out;
}

export function deriveBands(crossovers, count = 4) {
  const n = Math.round(clamp(asNumber(count, 4), 1, 4));
  const cross = Array.isArray(crossovers) ? crossovers : DEFAULT_CROSSOVERS;
  const edges = [BAND_MIN_HZ];
  for (let i = 0; i < n - 1; i += 1) {
    const value = asNumber(cross[i], DEFAULT_CROSSOVERS[i] || 1000);
    edges.push(clamp(value, BAND_MIN_HZ, BAND_MAX_HZ - BAND_MIN_WIDTH_HZ));
  }
  edges.push(BAND_TOP_HZ);
  const out = [];
  for (let i = 0; i < n; i += 1) {
    let low = edges[i];
    let high = edges[i + 1];
    if (high - low < BAND_MIN_WIDTH_HZ) high = Math.min(BAND_MAX_HZ, low + BAND_MIN_WIDTH_HZ);
    out.push({ lowHz: low, highHz: high, color: BAND_PALETTE[i] });
  }
  return out;
}

export function crossoversFromBands(bands) {
  const list = Array.isArray(bands) ? bands : [];
  const out = [];
  for (let i = 0; i < 3; i += 1) {
    out.push(list[i] ? list[i].highHz : DEFAULT_CROSSOVERS[i]);
  }
  return out;
}

/**
 * Validate the three crossover values used by `bandCount` bands.
 * Returns { ok, errorKey, bands }.
 */
export function validateCrossovers(crossovers, bandCount = 4) {
  const count = Math.round(clamp(asNumber(bandCount, 4), 1, 4));
  const values = [];
  for (let i = 0; i < count - 1; i += 1) {
    const value = asNumber(crossovers?.[i], NaN);
    if (!Number.isFinite(value) || value < BAND_MIN_HZ || value > BAND_MAX_HZ) {
      return { ok: false, errorKey: 'settings.crossoverError', bands: null };
    }
    values.push(value);
  }
  for (let i = 1; i < values.length; i += 1) {
    if (values[i] - values[i - 1] < BAND_MIN_WIDTH_HZ) {
      return { ok: false, errorKey: 'settings.crossoverError', bands: null };
    }
  }
  if (values.length > 0 && BAND_TOP_HZ - values[values.length - 1] < BAND_MIN_WIDTH_HZ) {
    return { ok: false, errorKey: 'settings.crossoverError', bands: null };
  }
  return { ok: true, errorKey: null, bands: deriveBands(values, count) };
}

/* ------------------------------------------------------------------ */
/* tempo / settings / project defaults                                 */
/* ------------------------------------------------------------------ */

export function defaultTempo() {
  return {
    enabled: false,
    bpm: 120,
    beatsPerBar: 4,
    barStartSec: 0,
    subdivision: 1,
    showGrid: true,
    snapToGrid: false,
  };
}

export function normalizeTempo(raw) {
  const d = defaultTempo();
  const meter = Number(raw?.beatsPerBar);
  const subdivision = Number(raw?.subdivision);
  return {
    enabled: asBool(raw?.enabled, d.enabled),
    bpm: Math.round(clamp(asNumber(raw?.bpm, d.bpm), 20, 400) * 10) / 10,
    beatsPerBar: METERS.includes(meter) ? meter : d.beatsPerBar,
    barStartSec: Math.max(0, asNumber(raw?.barStartSec, d.barStartSec)),
    subdivision: SUBDIVISIONS.includes(subdivision) ? subdivision : d.subdivision,
    showGrid: asBool(raw?.showGrid, d.showGrid),
    snapToGrid: asBool(raw?.snapToGrid, d.snapToGrid),
  };
}

export function defaultView() {
  return {
    zoomPxPerSec: DEFAULT_ZOOM,
    scrollSec: 0,
    selectedBand: 0,
    followPlayhead: true,
  };
}

export function normalizeView(raw) {
  const d = defaultView();
  return {
    zoomPxPerSec: clamp(asNumber(raw?.zoomPxPerSec, d.zoomPxPerSec), ZOOM_MIN, ZOOM_MAX),
    scrollSec: Math.max(0, asNumber(raw?.scrollSec, d.scrollSec)),
    selectedBand: Math.round(clamp(asNumber(raw?.selectedBand, d.selectedBand), 0, 3)),
    followPlayhead: asBool(raw?.followPlayhead, d.followPlayhead),
  };
}

export function defaultSettings() {
  return {
    schema: 'namivision.settings',
    version: 1,
    lang: 'ja',
    theme: 'dark',
    scale: 1,
    volume: 0.8,
    waveformGain: 1,
    followPlayhead: true,
    fftSize: 2048,
    bandCount: 4,
    bands: defaultBands(),
    onsetSensitivity: 0.5,
    csvDelimiter: ',',
    csvDecimals: 3,
    includeUndetected: true,
    autoSaveProject: false,
    lastProjectPath: null,
  };
}

export function normalizeSettings(raw) {
  const d = defaultSettings();
  const bandCount = Math.round(clamp(asNumber(raw?.bandCount, d.bandCount), 1, 4));
  let bands = Array.isArray(raw?.bands) ? normalizeBands(raw.bands, bandCount) : null;
  if (!bands || bands.length !== bandCount) {
    const cross = crossoversFromBands(Array.isArray(raw?.bands) ? raw.bands : d.bands);
    bands = deriveBands(cross, bandCount);
  }
  const fft = Number(raw?.fftSize);
  const scale = Math.round(clamp(asNumber(raw?.scale, d.scale), 0.8, 1.5) * 20) / 20;
  return {
    schema: 'namivision.settings',
    version: 1,
    lang: raw?.lang === 'en' ? 'en' : 'ja',
    theme: raw?.theme === 'light' ? 'light' : 'dark',
    scale,
    volume: clamp(asNumber(raw?.volume, d.volume), 0, 1),
    waveformGain: clamp(asNumber(raw?.waveformGain, d.waveformGain), 0.25, 4),
    followPlayhead: asBool(raw?.followPlayhead, d.followPlayhead),
    fftSize: FFT_SIZES.includes(fft) ? fft : d.fftSize,
    bandCount,
    bands,
    onsetSensitivity: clamp(asNumber(raw?.onsetSensitivity, d.onsetSensitivity), 0, 1),
    csvDelimiter: raw?.csvDelimiter === ';' ? ';' : ',',
    csvDecimals: Math.round(clamp(asNumber(raw?.csvDecimals, d.csvDecimals), 0, 6)),
    includeUndetected: asBool(raw?.includeUndetected, d.includeUndetected),
    autoSaveProject: asBool(raw?.autoSaveProject, d.autoSaveProject),
    lastProjectPath: typeof raw?.lastProjectPath === 'string' ? raw.lastProjectPath : null,
  };
}

export function normalizeUi(raw) {
  return {
    lang: raw?.lang === 'en' ? 'en' : 'ja',
    theme: raw?.theme === 'light' ? 'light' : 'dark',
    scale: Math.round(clamp(asNumber(raw?.scale, 1), 0.8, 1.5) * 20) / 20,
  };
}

export function makeAppError(code, params = {}) {
  return { code, message: '', params };
}

export function normalizeProject(raw) {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) {
    throw makeAppError('ERR_PROJECT_FORMAT');
  }
  if (raw.schema != null && raw.schema !== 'namivision.project') {
    throw makeAppError('ERR_PROJECT_FORMAT');
  }
  const version = asNumber(raw.version, 1);
  if (!Number.isFinite(version) || version < 1) throw makeAppError('ERR_PROJECT_FORMAT');
  if (version > 1) throw makeAppError('ERR_PROJECT_VERSION', { version: String(raw.version) });

  const audioRaw = raw.audio && typeof raw.audio === 'object' ? raw.audio : {};
  const path = asString(audioRaw.path, '');
  const fileName = asString(audioRaw.fileName, basename(path));
  const durationSec = Math.max(0, asNumber(audioRaw.durationSec, 0));

  const playbackRaw = raw.playback && typeof raw.playback === 'object' ? raw.playback : {};
  const bandRaw = Array.isArray(raw.bands) ? raw.bands : null;
  const bands = bandRaw
    ? normalizeBands(bandRaw, Math.round(clamp(bandRaw.length, 1, 4)))
    : defaultBands();

  return {
    schema: 'namivision.project',
    version: 1,
    appVersion: asString(raw.appVersion, '0.1.0'),
    savedAt: asString(raw.savedAt, ''),
    audio: {
      path,
      fileName,
      durationSec,
    },
    tempo: normalizeTempo(raw.tempo),
    view: normalizeView(raw.view),
    bands,
    playback: {
      positionSec: Math.max(0, asNumber(playbackRaw.positionSec, 0)),
      volume: clamp(asNumber(playbackRaw.volume, 0.8), 0, 1),
    },
    ui: normalizeUi(raw.ui),
    audioMissing: asBool(raw.audioMissing, false),
  };
}

export function normalizeLoadedProject(payload) {
  const source = payload && typeof payload === 'object' && payload.project ? payload.project : payload;
  const project = normalizeProject(source);
  const flag = payload && typeof payload === 'object' ? payload.audioMissing : undefined;
  return { ...project, audioMissing: asBool(flag, project.audioMissing) };
}

/* ------------------------------------------------------------------ */
/* beat deviation math (mirrors spec §4.7)                             */
/* ------------------------------------------------------------------ */

export function onsetThreshold(sensitivity) {
  return 0.3 - 0.25 * clamp(sensitivity, 0, 1);
}

/**
 * Compute one row per beat for a band.
 * Positive deviation = late (after the beat), negative = early.
 */
export function computeBeatDeviations({
  analysis,
  bandIndex = 0,
  tempo,
  sensitivity = 0.5,
  tolerance = BEAT_TOLERANCE,
} = {}) {
  const band = analysis?.bands?.[bandIndex];
  const onset = band?.onset;
  const hop = asNumber(analysis?.frameHopSec, 0);
  const duration = Math.max(0, asNumber(analysis?.meta?.durationSec, 0));
  if (!band || !Array.isArray(onset) || onset.length === 0 || !(hop > 0) || duration <= 0) {
    return [];
  }
  const t = normalizeTempo(tempo);
  const beatDur = beatDuration(t.bpm);
  const window = Math.max(0, asNumber(tolerance, BEAT_TOLERANCE)) * beatDur;
  const threshold = onsetThreshold(sensitivity);
  const barStart = clamp(t.barStartSec, 0, duration);
  const kMin = Math.ceil((0 - barStart) / beatDur - 1e-9);
  const kMax = Math.floor((duration - barStart) / beatDur + 1e-9);
  const rows = [];
  const lastFrame = onset.length - 1;

  for (let k = kMin; k <= kMax && rows.length < 20000; k += 1) {
    const expected = barStart + k * beatDur;
    if (expected < -1e-9 || expected > duration + 1e-9) continue;
    const from = Math.max(0, expected - window);
    const to = Math.min(duration, expected + window);
    let first = Math.floor(from / hop);
    let last = Math.ceil(to / hop);
    first = Math.max(0, Math.min(lastFrame, first));
    last = Math.max(0, Math.min(lastFrame, last));
    let bestFrame = -1;
    let best = -Infinity;
    for (let f = first; f <= last; f += 1) {
      const value = asNumber(onset[f], 0);
      if (value > best) {
        best = value;
        bestFrame = f;
      }
    }
    const row = {
      beatIndex: k,
      bar: barNumberForBeat(k, t.beatsPerBar),
      beatInBar: beatInBar(k, t.beatsPerBar),
      expectedTimeSec: expected,
      onsetTimeSec: null,
      deviationMs: null,
      deviationBeats: null,
      strength: null,
      status: 'undetected',
    };
    if (bestFrame < 0 || !(best >= threshold)) {
      rows.push(row);
      continue;
    }
    const yPrev = bestFrame > 0 ? asNumber(onset[bestFrame - 1], 0) : 0;
    const yPeak = best;
    const yNext = bestFrame < lastFrame ? asNumber(onset[bestFrame + 1], 0) : 0;
    const denom = yPrev - 2 * yPeak + yNext;
    let delta = 0;
    if (Math.abs(denom) > 1e-12) {
      delta = (0.5 * (yPrev - yNext)) / denom;
      if (!Number.isFinite(delta) || Math.abs(delta) > 1) delta = 0;
    }
    const onsetTime = (bestFrame + delta) * hop;
    row.onsetTimeSec = onsetTime;
    row.deviationMs = (onsetTime - expected) * 1000;
    row.deviationBeats = (onsetTime - expected) / beatDur;
    row.strength = best;
    row.status = 'detected';
    rows.push(row);
  }
  return rows;
}

export function csvField(value, delimiter = ',') {
  const text = value == null ? '' : String(value);
  if (text.includes(delimiter) || text.includes('"') || text.includes('\n') || text.includes('\r')) {
    return `"${text.replace(/"/g, '""')}"`;
  }
  return text;
}

export function buildBeatCsv({
  analysis,
  bandIndex = 0,
  tempo,
  sensitivity = 0.5,
  tolerance = BEAT_TOLERANCE,
  includeUndetected = true,
  delimiter = ',',
  decimals = 3,
} = {}) {
  const band = analysis?.bands?.[bandIndex];
  if (!band) return { csv: '', rows: 0, detected: 0 };
  const sep = delimiter === ';' ? ';' : ',';
  const places = Math.round(clamp(asNumber(decimals, 3), 0, 6));
  const rows = computeBeatDeviations({ analysis, bandIndex, tempo, sensitivity, tolerance });
  const header = [
    'band_index',
    'band_label',
    'frequency_range_hz',
    'beat_index',
    'bar',
    'beat_in_bar',
    'expected_time_s',
    'onset_time_s',
    'deviation_ms',
    'deviation_beats',
    'onset_strength',
    'status',
  ];
  const lines = [header.join(sep)];
  let detected = 0;
  for (const row of rows) {
    if (row.status !== 'detected' && !includeUndetected) continue;
    const fields = [
      csvField(band.index ?? bandIndex, sep),
      csvField(band.label, sep),
      csvField(`${band.lowHz}-${band.highHz}`, sep),
      row.beatIndex,
      row.bar,
      row.beatInBar,
      row.expectedTimeSec.toFixed(places),
    ];
    if (row.status === 'detected') {
      detected += 1;
      fields.push(
        row.onsetTimeSec.toFixed(places),
        row.deviationMs.toFixed(places),
        row.deviationBeats.toFixed(Math.min(6, places + 1)),
        row.strength.toFixed(places),
        'detected',
      );
    } else {
      fields.push('', '', '', '', 'undetected');
    }
    lines.push(fields.map((f) => csvField(f, sep)).join(sep));
  }
  return {
    csv: `\uFEFF${lines.join('\r\n')}\r\n`,
    rows: lines.length - 1,
    detected,
  };
}



