/**
 * NamiVision browser UI controller.
 *
 * The module owns one mutable state object, talks to either the Tauri bridge
 * or the bundled mock backend, and keeps the DOM, canvas and dialogs in sync.
 * All visible strings come from i18n.js.
 */

import {
  BEAT_TOLERANCE,
  BAND_LABELS,
  ZOOM_MIN,
  ZOOM_MAX,
  basename,
  beatDuration,
  clamp,
  clampView,
  crossoversFromBands,
  deriveBands,
  defaultTempo,
  fitView,
  formatAnalysisMs,
  formatBpm,
  formatHzRange,
  formatSampleRate,
  formatTime,
  normalizeLoadedProject,
  normalizeSettings,
  normalizeTempo,
  zoomViewAt,
} from './core.js';
import { applyI18n, hasKey, setLang, t } from './i18n.js';
import {
  createAppState,
  readUiCache,
  settingsFromState,
  writeUiCache,
} from './state.js';
import { TimelineRenderer } from './canvas.js';
import { ContextMenu } from './menus.js';
import {
  ConfirmDialog,
  DialogController,
  SettingsDialog,
  TempoDialog,
} from './dialogs.js';

const state = createAppState();
const dom = {};

let api = null;
let renderer = null;
let menu = null;
let dialogs = null;
let tempoDialog = null;
let settingsDialog = null;
let confirmDialog = null;

let settingsLoadFailed = false;
let saveTimer = 0;
let settingsSaveErrorShown = false;
let playbackPoll = 0;
let pollBusy = false;
let seekDragging = false;
let rulerDrag = false;
let flagDrag = null;
let lastNonZeroVolume = 0.8;
let resizeRaf = 0;

/* ------------------------------------------------------------------ */
/* backend adapter                                                     */
/* ------------------------------------------------------------------ */

async function initApi() {
  const tauri = window.__TAURI__;
  if (tauri && tauri.core && typeof tauri.core.invoke === 'function') {
    const listen = tauri.event && typeof tauri.event.listen === 'function'
      ? tauri.event.listen.bind(tauri.event)
      : async () => () => {};
    let onDragDrop = null;
    try {
      const webview = tauri.webviewWindow && typeof tauri.webviewWindow.getCurrentWebview === 'function'
        ? tauri.webviewWindow.getCurrentWebview()
        : null;
      if (webview && typeof webview.onDragDropEvent === 'function') {
        onDragDrop = webview.onDragDropEvent.bind(webview);
      }
    } catch {
      onDragDrop = null;
    }
    return { kind: 'tauri', invoke: tauri.core.invoke, listen, onDragDrop };
  }
  const mock = await import('../mock/mock-backend.js');
  return {
    kind: 'mock',
    invoke: mock.invoke,
    listen: mock.listen,
    onDragDrop: mock.onDragDropEvent,
  };
}

/* ------------------------------------------------------------------ */
/* small helpers                                                       */
/* ------------------------------------------------------------------ */

function stripBands(bands) {
  return (Array.isArray(bands) ? bands : []).map((band) => ({
    lowHz: Number(band.lowHz),
    highHz: Number(band.highHz),
    color: band.color,
  }));
}

function bandLabel(band, index) {
  return band && typeof band.label === 'string' && band.label
    ? band.label
    : BAND_LABELS[index] || t('lanes.bandFallback', { n: index + 1 });
}

function getDuration() {
  const value = Number(state.meta?.durationSec ?? state.playback.durationSec);
  return Number.isFinite(value) && value > 0 ? value : 0;
}

function maxScrollNow() {
  const width = renderer?.width || 0;
  const zoom = Number(state.view.zoomPxPerSec) || 96;
  return Math.max(0, getDuration() - width / zoom);
}

function isAudioFile(name) {
  return /\.(mp3|wav|flac|ogg|oga)$/i.test(String(name || ''));
}

function paintRange(input) {
  if (!input) return;
  const min = Number(input.min);
  const max = Number(input.max);
  const value = Number(input.value);
  const span = Number.isFinite(max) && Number.isFinite(min) ? max - min : 0;
  const percent = span > 0 && Number.isFinite(value)
    ? clamp(((value - min) / span) * 100, 0, 100)
    : 0;
  input.style.setProperty('--fill', `${percent}%`);
}

/* ------------------------------------------------------------------ */
/* preferences, layout and dynamic text                                */
/* ------------------------------------------------------------------ */

function applyPreferences(lang, theme, scale, options = {}) {
  const normalized = normalizeSettings({ ...state.settings, lang, theme, scale });
  state.settings.lang = normalized.lang;
  state.settings.theme = normalized.theme;
  state.settings.scale = normalized.scale;

  setLang(state.settings.lang);
  document.documentElement.lang = state.settings.lang;
  document.documentElement.dataset.theme = state.settings.theme;
  document.documentElement.style.colorScheme = state.settings.theme;
  document.documentElement.style.setProperty('--scale', String(state.settings.scale));
  document.documentElement.style.fontSize = `${14 * state.settings.scale}px`;
  if (options.writeCache !== false) {
    writeUiCache({
      lang: state.settings.lang,
      theme: state.settings.theme,
      scale: state.settings.scale,
    });
  }

  applyI18n(document);
  if (renderer) {
    renderer.setScale(state.settings.scale);
    renderer.invalidateTheme();
  }
  if (options.rebuildHeaders) refreshLaneHeaders();
  syncFileChip();
  syncTempoChip();
  syncPlaybackUI();
  syncStatus();
  syncEmptyState();
  updateRulerAria();
  updateHscroll();

  requestAnimationFrame(() => {
    if (!renderer) return;
    renderer.resize();
    relayout();
    state.view = clampView(state.view, getDuration(), renderer.width);
    renderer.requestDraw();
    updateHscroll();
  });
}

function refreshLaneHeaders() {
  if (!dom.laneHeaders) return;
  const selectedMaster = state.selectedLane === 'master';
  const master = document.createElement('button');
  master.type = 'button';
  master.className = 'lane-header is-master';
  master.dataset.lane = 'master';
  master.setAttribute('aria-pressed', selectedMaster ? 'true' : 'false');
  const masterSwatch = document.createElement('span');
  masterSwatch.className = 'lane-swatch';
  masterSwatch.setAttribute('aria-hidden', 'true');
  const masterCopy = document.createElement('span');
  masterCopy.className = 'lane-header-copy';
  const masterName = document.createElement('span');
  masterName.className = 'lane-name';
  masterName.textContent = t('lanes.master');
  const masterSub = document.createElement('span');
  masterSub.className = 'lane-range';
  masterSub.textContent = t('lanes.masterSub');
  masterCopy.append(masterName, masterSub);
  master.append(masterSwatch, masterCopy);
  master.addEventListener('click', () => selectLane('master'));
  dom.laneHeaders.replaceChildren(master);

  state.bands.forEach((band, index) => {
    const range = formatHzRange(band.lowHz, band.highHz);
    const label = bandLabel(band, index);
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'lane-header';
    button.dataset.lane = 'band';
    button.dataset.band = String(index);
    button.style.setProperty('--lane-color', band.color || 'var(--muted)');
    button.setAttribute(
      'aria-label',
      t('lanes.bandAria', { label, range }),
    );
    button.setAttribute(
      'aria-pressed',
      state.selectedLane === 'band' && state.selectedBand === index ? 'true' : 'false',
    );
    const swatch = document.createElement('span');
    swatch.className = 'lane-swatch';
    swatch.setAttribute('aria-hidden', 'true');
    const copy = document.createElement('span');
    copy.className = 'lane-header-copy';
    const name = document.createElement('span');
    name.className = 'lane-name';
    name.textContent = label;
    const rangeEl = document.createElement('span');
    rangeEl.className = 'lane-range';
    rangeEl.textContent = range;
    copy.append(name, rangeEl);
    button.append(swatch, copy);
    button.addEventListener('click', () => selectLane('band', index));
    dom.laneHeaders.appendChild(button);
  });
  updateLaneHeaderSelection();
  relayout();
}

function updateLaneHeaderSelection() {
  if (!dom.laneHeaders) return;
  for (const header of dom.laneHeaders.querySelectorAll('.lane-header')) {
    const isMaster = header.dataset.lane === 'master';
    const index = Number(header.dataset.band);
    const selected = isMaster
      ? state.selectedLane === 'master'
      : state.selectedLane === 'band' && state.selectedBand === index;
    header.classList.toggle('is-selected', selected);
    header.setAttribute('aria-pressed', selected ? 'true' : 'false');
  }
}

function relayout() {
  if (!renderer || !dom.laneView) return;
  const total = Math.round(dom.laneView.clientHeight);
  const count = Math.max(1, state.bands.length);
  if (!(total > 0)) return;
  const scale = state.settings.scale || 1;
  const minMaster = Math.round(96 * scale);
  const minBand = Math.round(56 * scale);
  let master = Math.max(minMaster, Math.round(total * 0.34));
  if (total - master < count * minBand) {
    master = Math.max(Math.round(72 * scale), total - count * minBand);
  }
  if (master > total - count * 40) master = Math.round(total * 0.34);
  master = clamp(master, 1, Math.max(1, total - count));
  const bandHeight = (total - master) / count;
  renderer.setLayout([master, ...Array(count).fill(bandHeight)]);
  const bounds = renderer.bounds();
  const headers = dom.laneHeaders ? [...dom.laneHeaders.children] : [];
  headers.forEach((header, index) => {
    if (bounds[index]) header.style.height = `${bounds[index].height}px`;
  });
}

function syncAll() {
  if (dom.laneHeaders && dom.laneHeaders.children.length !== state.bands.length + 1) {
    refreshLaneHeaders();
  }
  syncFileChip();
  syncTempoChip();
  syncPlaybackUI();
  syncStatus();
  syncEmptyState();
  updateHscroll();
  updateRulerAria();
  updateLaneHeaderSelection();
}

function syncFileChip() {
  if (!dom.fileChip) return;
  const meta = state.meta;
  const projectAudio = state.projectAudio;
  dom.fileChip.classList.toggle('has-audio', Boolean(meta && meta.fileName));
  dom.fileChip.classList.toggle('is-missing', Boolean(!meta && projectAudio && state.audioMissing));
  if (meta && meta.fileName) {
    dom.fileName.textContent = meta.fileName;
    dom.fileDuration.textContent = formatTime(meta.durationSec, { decimals: 2 });
    dom.fileChip.title = meta.path || meta.fileName;
  } else if (projectAudio && projectAudio.fileName) {
    dom.fileName.textContent = projectAudio.fileName;
    dom.fileDuration.textContent = formatTime(projectAudio.durationSec, { decimals: 2 });
    dom.fileChip.title = t('header.fileMissingHint', { path: projectAudio.path || projectAudio.fileName });
  } else {
    dom.fileName.textContent = t('header.noAudio');
    dom.fileDuration.textContent = '';
    dom.fileChip.title = t('header.noAudioHint');
  }
}

function syncTempoChip() {
  if (!dom.tempoChip) return;
  const tempo = state.tempo;
  if (tempo && tempo.enabled) {
    const value = t('transport.tempoChip', {
      bpm: formatBpm(tempo.bpm),
      meter: tempo.beatsPerBar,
    });
    dom.tempoChipText.textContent = value;
    dom.tempoChip.classList.remove('is-unset');
    dom.tempoChip.setAttribute('aria-label', t('transport.tempoAria', { value }));
    dom.tempoChip.title = t('transport.tempo');
  } else {
    dom.tempoChipText.textContent = t('transport.tempoNotSet');
    dom.tempoChip.classList.add('is-unset');
    dom.tempoChip.setAttribute('aria-label', t('transport.tempo'));
    dom.tempoChip.title = t('transport.tempo');
  }
}

function syncStatus() {
  if (!dom.statusSr) return;
  const meta = state.meta;
  const duration = getDuration();
  dom.statusSr.textContent = meta ? formatSampleRate(meta.sampleRate) : '—';
  dom.statusCh.textContent = meta ? t('status.chUnit', { n: meta.channels }) : '—';
  dom.statusDuration.textContent = duration > 0 ? formatTime(duration, { decimals: 2 }) : '—';
  dom.statusAnalysis.textContent = state.analysis ? formatAnalysisMs(state.analysis.analysisMs) : '—';
  dom.statusZoom.textContent = duration > 0
    ? t('status.zoomUnit', { n: Math.round(state.view.zoomPxPerSec) })
    : '—';
  if (state.selectedLane === 'master') {
    dom.statusLane.textContent = t('status.master');
  } else {
    const band = state.bands[state.selectedBand];
    dom.statusLane.textContent = band
      ? `${bandLabel(band, state.selectedBand)} · ${formatHzRange(band.lowHz, band.highHz)}`
      : '—';
  }
}

function syncEmptyState() {
  if (!dom.emptyState) return;
  const missing = Boolean(!state.analysis && state.audioMissing);
  const hide = Boolean(state.analysis) || state.loading || state.audioMissing;
  dom.emptyState.hidden = hide;
  if (dom.missingBanner) dom.missingBanner.hidden = !missing;
  if (missing && dom.missingPath) {
    dom.missingPath.textContent = state.missingAudioPath || state.projectAudio?.path || '';
  }
  if (dom.loadingOverlay) dom.loadingOverlay.hidden = !state.loading;
  if (dom.app) dom.app.setAttribute('aria-busy', state.loading ? 'true' : 'false');
}

function updateRulerAria() {
  if (!dom.rulerCanvas) return;
  const duration = getDuration();
  const position = clamp(Number(state.playback.positionSec) || 0, 0, duration);
  dom.rulerCanvas.setAttribute('aria-valuemax', String(Math.round(duration * 1000)));
  dom.rulerCanvas.setAttribute('aria-valuenow', String(Math.round(position * 1000)));
  dom.rulerCanvas.setAttribute('aria-valuetext', formatTime(position, { decimals: 2 }));
}

function updateHscroll() {
  if (!dom.hscroll || !dom.hscrollThumb) return;
  const width = renderer?.width || 0;
  const duration = getDuration();
  const zoom = Number(state.view.zoomPxPerSec) || 96;
  const maxScroll = Math.max(0, duration - width / zoom);
  const overflow = width > 0 && duration > 0 && maxScroll > 0.001;
  dom.hscroll.hidden = !overflow;
  if (!overflow) return;
  const trackWidth = Math.max(1, dom.hscroll.clientWidth);
  const visible = Math.min(duration, width / zoom);
  const thumbWidth = clamp((trackWidth * visible) / duration, 24, trackWidth);
  const left = maxScroll > 0
    ? (state.view.scrollSec / maxScroll) * (trackWidth - thumbWidth)
    : 0;
  dom.hscrollThumb.style.width = `${thumbWidth}px`;
  dom.hscrollThumb.style.left = `${left}px`;
  dom.hscrollThumb.setAttribute('aria-valuemax', String(Math.round(maxScroll * 1000)));
  dom.hscrollThumb.setAttribute('aria-valuenow', String(Math.round(state.view.scrollSec * 1000)));
  dom.hscrollThumb.setAttribute('aria-valuetext', formatTime(state.view.scrollSec, { decimals: 2 }));
}

function setProgress(payload = {}) {
  const percent = Math.round(clamp(Number(payload.progress) || 0, 0, 1) * 100);
  if (dom.loadingPercent) dom.loadingPercent.textContent = t('loading.percent', { n: percent });
  if (dom.loadingBar) dom.loadingBar.style.width = `${percent}%`;
  const order = ['decode', 'fft', 'bands'];
  const index = Math.max(0, order.indexOf(payload.phase));
  (dom.loadingPhases || []).forEach((phase, i) => {
    phase.classList.toggle('is-active', i === index);
    phase.classList.toggle('is-done', i < index);
  });
}

function showLoading(active) {
  state.loading = Boolean(active);
  if (dom.loadingOverlay) dom.loadingOverlay.hidden = !state.loading;
  if (dom.app) dom.app.setAttribute('aria-busy', state.loading ? 'true' : 'false');
  if (state.loading) setProgress({ phase: 'decode', progress: 0 });
  syncEmptyState();
}

/* ------------------------------------------------------------------ */
/* toasts                                                              */
/* ------------------------------------------------------------------ */

function removeToast(element) {
  if (!element || element.dataset.leaving === '1') return;
  element.dataset.leaving = '1';
  if (element._nvTimer) {
    clearTimeout(element._nvTimer);
    element._nvTimer = 0;
  }
  element.classList.add('is-leaving');
  setTimeout(() => element.remove(), 180);
}

function showToast(message, options = {}) {
  const region = dom.toastRegion || document.getElementById('toast-region');
  if (!region) return null;
  const type = ['success', 'error', 'warning', 'info'].includes(options.type)
    ? options.type
    : 'info';
  const element = document.createElement('div');
  element.className = `toast is-${type}`;
  element.setAttribute('role', type === 'error' ? 'alert' : 'status');

  const icon = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  icon.setAttribute('class', 'icon toast-icon');
  icon.setAttribute('aria-hidden', 'true');
  const use = document.createElementNS('http://www.w3.org/2000/svg', 'use');
  const iconId = type === 'success' ? 'icon-check'
    : type === 'error' ? 'icon-error'
      : type === 'warning' ? 'icon-warning'
        : 'icon-info';
  use.setAttribute('href', `#${iconId}`);
  icon.appendChild(use);

  const body = document.createElement('div');
  body.className = 'toast-body';
  const text = document.createElement('p');
  text.className = 'toast-message';
  text.textContent = String(message == null ? '' : message);
  body.appendChild(text);

  if (options.action && typeof options.action.onClick === 'function') {
    const action = document.createElement('button');
    action.type = 'button';
    action.className = 'toast-action';
    action.textContent = options.action.label || '';
    action.addEventListener('click', () => {
      removeToast(element);
      try {
        options.action.onClick();
      } catch {
        /* action failures must not break the toast region */
      }
    });
    body.appendChild(action);
  }

  const close = document.createElement('button');
  close.type = 'button';
  close.className = 'toast-close';
  close.title = t('toast.close');
  close.setAttribute('aria-label', t('toast.close'));
  const closeIcon = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  closeIcon.setAttribute('class', 'icon');
  closeIcon.setAttribute('aria-hidden', 'true');
  const closeUse = document.createElementNS('http://www.w3.org/2000/svg', 'use');
  closeUse.setAttribute('href', '#icon-close');
  closeIcon.appendChild(closeUse);
  close.appendChild(closeIcon);
  close.addEventListener('click', () => removeToast(element));

  element.append(icon, body, close);
  region.appendChild(element);
  while (region.children.length > 4) {
    const oldest = region.firstElementChild;
    if (!oldest) break;
    clearTimeout(oldest._nvTimer);
    oldest._nvTimer = 0;
    oldest.remove();
  }

  const timeout = options.timeout == null ? 5200 : Number(options.timeout);
  if (timeout > 0) {
    element._nvTimer = setTimeout(() => removeToast(element), timeout);
  }
  return element;
}

function errorToast(error, fallbackKey) {
  const code = error && typeof error === 'object' ? error.code : null;
  const params = error && typeof error === 'object' && error.params ? error.params : {};
  if (code && hasKey(`error.${code}`)) {
    showToast(t(`error.${code}`, params), { type: 'error', timeout: 7000 });
    return;
  }
  const message = error && typeof error === 'object' && error.message
    ? error.message
    : String(error || '');
  if (fallbackKey && hasKey(fallbackKey)) {
    showToast(t(fallbackKey), { type: 'error', timeout: 7000 });
  } else {
    showToast(t('error.ERR_UNKNOWN', { message }), { type: 'error', timeout: 7000 });
  }
}

/* ------------------------------------------------------------------ */
/* playback                                                            */
/* ------------------------------------------------------------------ */

function syncPlaybackUI() {
  if (!dom.playBtn) return;
  const duration = getDuration();
  const hasAudio = Boolean(state.analysis);
  const position = clamp(Number(state.playback.positionSec) || 0, 0, duration);
  const playing = Boolean(state.playback.playing);

  dom.playBtn.disabled = !hasAudio;
  dom.stopBtn.disabled = !hasAudio;
  dom.seek.disabled = !hasAudio;
  dom.playBtn.classList.toggle('is-playing', playing);
  dom.playBtn.setAttribute('aria-pressed', playing ? 'true' : 'false');
  if (dom.playUse) dom.playUse.setAttribute('href', playing ? '#icon-pause' : '#icon-play');
  dom.playLabel.textContent = playing ? t('transport.pause') : t('transport.play');
  dom.playBtn.title = hasAudio
    ? (playing ? t('transport.pause') : t('transport.play'))
    : t('transport.noAudio');

  dom.timeCurrent.textContent = formatTime(position, { decimals: 2 });
  dom.timeTotal.textContent = formatTime(duration, { decimals: 2 });
  dom.seek.max = String(Math.max(0.01, duration || 0));
  if (!seekDragging) dom.seek.value = String(position);
  dom.seek.setAttribute('aria-valuetext', formatTime(position, { decimals: 2 }));
  paintRange(dom.seek);

  const volume = clamp(Number(state.playback.volume) || 0, 0, 1);
  if (dom.volume) dom.volume.value = String(volume);
  paintRange(dom.volume);
  const muted = volume <= 0.001;
  if (dom.volumeUse) {
    dom.volumeUse.setAttribute(
      'href',
      muted ? '#icon-volume-mute' : volume < 0.5 ? '#icon-volume-low' : '#icon-volume',
    );
  }
  const volumeLabel = muted ? t('transport.unmute') : t('transport.mute');
  dom.volumeBtn.title = volumeLabel;
  dom.volumeBtn.setAttribute('aria-label', volumeLabel);

  updateRulerAria();
}

function applyPlaybackState(next) {
  if (!next || typeof next !== 'object') return;
  const ended = Boolean(next.ended);
  if (ended) {
    // Latch the UI at the start of the track.  Do not seek the backend here:
    // that would clear the `ended` flag before a client can observe it, and
    // `play` already restarts from 0 when the backend is in the ended state.
    state.playback.ended = true;
    state.playback.playing = false;
    state.playback.positionSec = 0;
    syncPlaybackUI();
    if (renderer) renderer.requestDraw();
    return;
  }
  state.playback.ended = false;
  state.playback.playing = Boolean(next.playing);
  if (Number.isFinite(Number(next.positionSec))) {
    state.playback.positionSec = clamp(Number(next.positionSec), 0, getDuration());
  }
  if (Number.isFinite(Number(next.durationSec)) && Number(next.durationSec) > 0) {
    state.playback.durationSec = Number(next.durationSec);
  }
  if (Number.isFinite(Number(next.volume))) {
    state.playback.volume = clamp(Number(next.volume), 0, 1);
  }
  syncPlaybackUI();
  if (renderer) renderer.requestDraw();
  ensurePlayheadVisible();
}

function startPlaybackPolling() {
  if (playbackPoll) clearInterval(playbackPoll);
  playbackPoll = setInterval(async () => {
    if (!api || pollBusy || !state.analysis) return;
    pollBusy = true;
    try {
      const next = await api.invoke('playback_state');
      applyPlaybackState(next);
    } catch {
      /* playback polling is best effort */
    } finally {
      pollBusy = false;
    }
  }, 120);
}

async function togglePlay() {
  if (!state.analysis) {
    showToast(t('toast.noAudio'), { type: 'warning' });
    return;
  }
  if (!api) return;
  try {
    if (state.playback.playing) {
      await api.invoke('pause');
      state.playback.playing = false;
    } else {
      await api.invoke('play');
      state.playback.playing = true;
      state.playback.ended = false;
    }
    syncPlaybackUI();
    if (renderer) renderer.requestDraw();
    ensurePlayheadVisible(true);
  } catch (error) {
    if (error && error.code === 'ERR_AUDIO_DEVICE') {
      showToast(t('toast.audioDevice'), { type: 'warning', timeout: 7000 });
    } else {
      errorToast(error);
    }
  }
}

async function stopPlayback() {
  if (!state.analysis || !api) return;
  try {
    await api.invoke('stop');
  } catch {
    /* local state still resets below */
  }
  state.playback.playing = false;
  state.playback.ended = false;
  state.playback.positionSec = 0;
  syncPlaybackUI();
  if (renderer) renderer.requestDraw();
}

function seekTo(seconds, options = {}) {
  const duration = getDuration();
  if (!(duration > 0)) return;
  const position = clamp(Number(seconds) || 0, 0, duration);
  state.playback.positionSec = position;
  state.playback.ended = false;
  syncPlaybackUI();
  if (renderer) renderer.requestDraw();
  if (options.commit !== false && api) {
    api.invoke('seek', { seconds: position }).catch(() => {});
  }
}

function setVolume(value, options = {}) {
  const volume = clamp(Number(value) || 0, 0, 1);
  state.playback.volume = volume;
  state.settings.volume = volume;
  if (volume > 0.001) lastNonZeroVolume = volume;
  syncPlaybackUI();
  if (options.commit !== false && api) {
    api.invoke('set_volume', { volume }).catch((error) => {
      if (error && error.code === 'ERR_AUDIO_DEVICE') {
        showToast(t('toast.audioDevice'), { type: 'warning', timeout: 7000 });
      }
    });
  }
  scheduleSaveSettings();
}

function toggleMute() {
  if (state.playback.volume > 0.001) {
    setVolume(0);
  } else {
    setVolume(lastNonZeroVolume || 0.8);
  }
}

/* ------------------------------------------------------------------ */
/* view / zoom / scroll                                                */
/* ------------------------------------------------------------------ */

function afterViewChange() {
  state.view = clampView(state.view, getDuration(), renderer?.width || 0);
  if (renderer) renderer.requestDraw();
  updateHscroll();
  syncStatus();
  updateRulerAria();
  state.dirty.project = true;
}

function zoomAnchor() {
  const position = Number(state.playback.positionSec) || 0;
  const width = renderer?.width || 0;
  const zoom = Number(state.view.zoomPxPerSec) || 96;
  const visible = width / zoom;
  const start = state.view.scrollSec;
  if (position >= start && position <= start + visible) return position;
  return start + visible / 2;
}

function zoomAt(factor, anchor) {
  const duration = getDuration();
  if (!(duration > 0)) return;
  const width = renderer?.width || dom.laneView.clientWidth || 1;
  const anchorSec = Number.isFinite(Number(anchor)) ? Number(anchor) : zoomAnchor();
  const nextZoom = clamp(state.view.zoomPxPerSec * factor, ZOOM_MIN, ZOOM_MAX);
  state.view = zoomViewAt(state.view, anchorSec, nextZoom, duration, width);
  afterViewChange();
}

function fitViewNow() {
  const duration = getDuration();
  if (!(duration > 0)) return;
  const width = renderer?.width || dom.laneView.clientWidth || 1;
  state.view = fitView(duration, width, {
    padding: 16,
    selectedBand: state.view.selectedBand,
  });
  state.view.followPlayhead = state.settings.followPlayhead;
  afterViewChange();
}

function ensurePlayheadVisible(force = false) {
  if (!state.view.followPlayhead || !state.playback.playing) return;
  const width = renderer?.width || 0;
  const duration = getDuration();
  if (!(width > 0) || !(duration > 0)) return;
  const zoom = Number(state.view.zoomPxPerSec) || 96;
  const visible = width / zoom;
  const position = Number(state.playback.positionSec) || 0;
  const start = state.view.scrollSec;
  const end = start + visible;
  const margin = Math.min(visible * 0.12, 1.2);
  if (force || position < start + margin * 0.25 || position > end - margin) {
    const target = clamp(position - visible * 0.18, 0, Math.max(0, duration - visible));
    if (Math.abs(target - start) > 0.005) {
      state.view.scrollSec = target;
      afterViewChange();
    }
  }
}

/* ------------------------------------------------------------------ */
/* settings persistence                                                */
/* ------------------------------------------------------------------ */

function scheduleSaveSettings() {
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    saveTimer = 0;
    saveSettingsNow();
  }, 400);
}

async function saveSettingsNow() {
  if (!api) return;
  try {
    await api.invoke('save_settings', { settings: settingsFromState(state) });
    settingsSaveErrorShown = false;
  } catch (error) {
    if (!settingsSaveErrorShown) {
      settingsSaveErrorShown = true;
      errorToast(error, 'toast.settingsSaveFailed');
    }
  }
}

/* ------------------------------------------------------------------ */
/* analysis / audio                                                    */
/* ------------------------------------------------------------------ */

function applyAnalysis(result, options = {}) {
  if (!result || !result.meta) return;
  const duration = Math.max(0, Number(result.meta.durationSec) || 0);
  const resetTempo = options.resetTempo !== false;
  const resetView = options.resetView !== false;
  const preserveSelection = options.preserveSelection === true;

  state.analysis = result;
  state.meta = result.meta;
  state.projectAudio = null;
  state.audioMissing = false;
  state.missingAudioPath = '';
  state.playback.durationSec = duration;
  state.playback.positionSec = clamp(Number(options.position) || 0, 0, duration);
  state.playback.ended = false;
  if (resetTempo) state.tempo = defaultTempo();

  const incoming = Array.isArray(result.bands) && result.bands.length
    ? result.bands
    : state.settings.bands;
  state.bands = incoming.map((band, index) => ({
    ...band,
    index,
    label: bandLabel(band, index),
    color: band.color || `var(--band-${Math.min(3, index)})`,
  }));
  state.settings.bands = stripBands(state.bands);
  state.settings.bandCount = state.bands.length;
  state.crossovers = crossoversFromBands(state.bands);

  if (!preserveSelection) {
    state.selectedLane = 'master';
    state.selectedBand = 0;
  }
  state.selectedBand = clamp(state.selectedBand, 0, Math.max(0, state.bands.length - 1));

  const width = renderer?.width || dom.laneView.clientWidth || 0;
  if (resetView) {
    state.view = fitView(duration, width, {
      padding: 16,
      selectedBand: preserveSelection ? state.view.selectedBand : 0,
    });
    state.view.followPlayhead = state.settings.followPlayhead;
  } else {
    state.view = clampView(
      { ...state.view, selectedBand: state.selectedBand },
      duration,
      width,
    );
  }

  if (renderer) {
    renderer.invalidateData();
    renderer.requestDraw();
  }
  refreshLaneHeaders();
  relayout();
  syncAll();
  if (api) api.invoke('seek', { seconds: state.playback.positionSec }).catch(() => {});
  if (result.warning === 'longAudio') {
    showToast(t('toast.longAudio'), { type: 'warning', timeout: 8000 });
  }
}

async function loadAudio(path, options = {}) {
  if (!api || state.loading) return false;
  const resetTempo = options.resetTempo !== false;
  const resetView = options.resetView !== false;
  const position = Number(options.position) || 0;
  const preserveSelection = options.preserveSelection === true;
  const bands = Array.isArray(options.bands) ? options.bands : null;
  showLoading(true);
  try {
    const result = await api.invoke('load_audio', { path });
    if (bands && bands.length) {
      try {
        const bandResult = await api.invoke('set_bands', {
          bands,
          fftSize: state.settings.fftSize,
        });
        if (bandResult && Array.isArray(bandResult.bands) && bandResult.bands.length) {
          result.bands = bandResult.bands;
        }
      } catch {
        /* the analysis result already has usable bands */
      }
    }
    state.playback.playing = false;
    applyAnalysis(result, {
      resetTempo,
      resetView,
      position,
      preserveSelection,
    });
    if (options.toast !== false) {
      showToast(
        t('toast.audioLoaded', {
          name: result.meta.fileName,
          duration: formatTime(result.meta.durationSec, { decimals: 2 }),
        }),
        { type: 'success' },
      );
    }
    if (options.relinked) {
      showToast(t('project.relinked'), { type: 'success' });
    }
    scheduleSaveSettings();
    return true;
  } catch (error) {
    errorToast(error, options.relinked ? 'toast.relinkFailed' : 'toast.audioLoadFailed');
    return false;
  } finally {
    showLoading(false);
  }
}

async function openAudio() {
  if (!api || state.loading) return;
  try {
    const picked = await api.invoke('pick_audio');
    if (!picked || picked.canceled || !picked.path) return;
    await loadAudio(picked.path);
  } catch (error) {
    errorToast(error, 'toast.audioLoadFailed');
  }
}

async function relinkAudio() {
  if (!api || state.loading) return;
  const keep = {
    bands: stripBands(state.bands),
    tempo: { ...state.tempo },
    view: { ...state.view },
    position: state.playback.positionSec,
    selectedLane: state.selectedLane,
    selectedBand: state.selectedBand,
  };
  try {
    const picked = await api.invoke('pick_audio');
    if (!picked || picked.canceled || !picked.path) return;
    const ok = await loadAudio(picked.path, {
      resetTempo: false,
      resetView: false,
      bands: keep.bands,
      position: keep.position,
      preserveSelection: true,
      toast: false,
    });
    if (!ok) return;
    state.tempo = keep.tempo;
    state.view = keep.view;
    state.selectedLane = keep.selectedLane;
    state.selectedBand = keep.selectedBand;
    state.view.selectedBand = keep.selectedBand;
    state.projectAudio = null;
    state.audioMissing = false;
    state.missingAudioPath = '';
    if (renderer) renderer.requestDraw();
    refreshLaneHeaders();
    relayout();
    syncAll();
    showToast(t('project.relinked'), { type: 'success' });
    scheduleSaveSettings();
  } catch (error) {
    errorToast(error, 'toast.relinkFailed');
  }
}

/* ------------------------------------------------------------------ */
/* projects                                                            */
/* ------------------------------------------------------------------ */

function buildProject() {
  const audio = state.analysis?.meta || state.projectAudio || {};
  return {
    schema: 'namivision.project',
    version: 1,
    appVersion: state.appInfo?.version || '0.1.0',
    savedAt: new Date().toISOString(),
    audio: {
      path: audio.path || '',
      fileName: audio.fileName || '',
      durationSec: Number(audio.durationSec) || 0,
    },
    tempo: { ...state.tempo },
    view: { ...state.view },
    bands: stripBands(state.bands),
    playback: {
      positionSec: state.playback.positionSec,
      volume: state.playback.volume,
    },
    ui: {
      lang: state.settings.lang,
      theme: state.settings.theme,
      scale: state.settings.scale,
    },
  };
}

async function saveProject() {
  if (!api) return;
  if (!state.analysis && !state.projectAudio) {
    showToast(t('project.needAudio'), { type: 'warning' });
    return;
  }
  try {
    const result = await api.invoke('save_project', { project: buildProject() });
    if (!result || result.canceled || !result.path) return;
    state.projectPath = result.path;
    state.settings.lastProjectPath = result.path;
    scheduleSaveSettings();
    showToast(t('project.saved', { name: basename(result.path) }), { type: 'success' });
  } catch (error) {
    errorToast(error, 'toast.projectSaveFailed');
  }
}

async function applyProject(project, path) {
  state.projectPath = path || null;
  state.settings.lastProjectPath = path || null;
  state.settings.lang = project.ui.lang;
  state.settings.theme = project.ui.theme;
  state.settings.scale = project.ui.scale;
  state.settings.volume = project.playback.volume;
  state.settings.bands = stripBands(project.bands);
  state.settings.bandCount = project.bands.length;
  state.crossovers = crossoversFromBands(project.bands);
  state.bands = project.bands.map((band, index) => ({
    ...band,
    index,
    label: bandLabel(band, index),
    color: band.color || `var(--band-${Math.min(3, index)})`,
  }));
  applyPreferences(project.ui.lang, project.ui.theme, project.ui.scale, {
    rebuildHeaders: true,
  });
  state.tempo = { ...project.tempo };
  state.playback.volume = project.playback.volume;
  state.playback.playing = false;
  state.playback.ended = false;

  if (project.audioMissing || !project.audio.path) {
    state.analysis = null;
    state.meta = null;
    state.projectAudio = { ...project.audio };
    state.audioMissing = true;
    state.missingAudioPath = project.audio.path || project.audio.fileName || '';
    state.playback.durationSec = Number(project.audio.durationSec) || 0;
    state.playback.positionSec = 0;
    state.selectedLane = 'master';
    state.selectedBand = 0;
    state.view = clampView(project.view, getDuration(), renderer?.width || 0);
    if (renderer) renderer.invalidateData();
    refreshLaneHeaders();
    relayout();
    syncAll();
    if (renderer) renderer.requestDraw();
    showToast(t('toast.projectLoadedMissing'), { type: 'warning', timeout: 8000 });
    scheduleSaveSettings();
    return;
  }

  const ok = await loadAudio(project.audio.path, {
    resetTempo: false,
    resetView: false,
    bands: stripBands(project.bands),
    position: project.playback.positionSec,
    preserveSelection: true,
    toast: false,
  });
  if (!ok) {
    state.analysis = null;
    state.meta = null;
    state.projectAudio = { ...project.audio };
    state.audioMissing = true;
    state.missingAudioPath = project.audio.path;
    state.playback.durationSec = Number(project.audio.durationSec) || 0;
    state.playback.positionSec = 0;
    state.bands = project.bands.map((band, index) => ({
      ...band,
      index,
      label: bandLabel(band, index),
      color: band.color || `var(--band-${Math.min(3, index)})`,
    }));
    state.tempo = { ...project.tempo };
    state.view = clampView(project.view, getDuration(), renderer?.width || 0);
    if (renderer) renderer.invalidateData();
    refreshLaneHeaders();
    relayout();
    syncAll();
    showToast(t('toast.projectLoadedMissing'), { type: 'warning', timeout: 8000 });
    return;
  }

  state.tempo = { ...project.tempo };
  state.view = clampView(project.view, getDuration(), renderer?.width || 0);
  state.playback.volume = project.playback.volume;
  state.settings.volume = project.playback.volume;
  state.playback.positionSec = clamp(project.playback.positionSec, 0, getDuration());
  state.selectedBand = clamp(state.view.selectedBand, 0, state.bands.length - 1);
  state.selectedLane = 'master';
  state.view.selectedBand = state.selectedBand;
  if (renderer) renderer.requestDraw();
  refreshLaneHeaders();
  relayout();
  syncAll();
  showToast(t('project.loaded', { name: basename(path) }), { type: 'success' });
  scheduleSaveSettings();
}

async function loadProjectFromPath(path) {
  if (!api) return;
  try {
    const payload = await api.invoke('load_project', { path });
    const project = normalizeLoadedProject(payload);
    await applyProject(project, path);
  } catch (error) {
    errorToast(error, 'toast.projectLoadFailed');
  }
}

async function loadProject() {
  if (!api || state.loading) return;
  try {
    const picked = await api.invoke('pick_project');
    if (!picked || picked.canceled || !picked.path) return;
    await loadProjectFromPath(picked.path);
  } catch (error) {
    errorToast(error, 'toast.projectLoadFailed');
  }
}

/* ------------------------------------------------------------------ */
/* CSV export                                                          */
/* ------------------------------------------------------------------ */

async function exportBand(bandIndex) {
  if (!api || state.exporting) return;
  if (!state.analysis) {
    showToast(t('toast.noAudio'), { type: 'warning' });
    return;
  }
  if (!state.tempo.enabled) {
    showToast(t('toast.barStartNeedsTempo'), { type: 'warning' });
    return;
  }
  const band = state.bands[bandIndex];
  if (!band) return;
  state.exporting = true;
  const pending = showToast(t('toast.exporting'), { type: 'info', timeout: 0 });
  try {
    const result = await api.invoke('export_beats_csv', {
      bandIndex,
      tempo: state.tempo,
      tolerance: BEAT_TOLERANCE,
      includeUndetected: state.settings.includeUndetected,
      delimiter: state.settings.csvDelimiter,
      decimals: state.settings.csvDecimals,
    });
    removeToast(pending);
    if (!result || result.canceled) return;
    showToast(
      t('toast.csvExported', { rows: result.rows, detected: result.detected }),
      {
        type: 'success',
        timeout: 9000,
        action: {
          label: t('toast.openFolder'),
          onClick: () => {
            api.invoke('reveal_file', { path: result.path }).catch(() => {});
          },
        },
      },
    );
  } catch (error) {
    removeToast(pending);
    errorToast(error, 'toast.csvFailed');
  } finally {
    state.exporting = false;
  }
}

/* ------------------------------------------------------------------ */
/* tempo and context menus                                             */
/* ------------------------------------------------------------------ */

function openTempo() {
  if (!(getDuration() > 0)) {
    showToast(t('tempo.errorNoAudio'), { type: 'warning' });
    return;
  }
  tempoDialog.open(state.tempo, {
    duration: getDuration(),
    playheadSec: state.playback.positionSec,
  });
}

function applyTempo(tempo) {
  state.tempo = normalizeTempo(tempo);
  state.tempo.enabled = true;
  state.dirty.project = true;
  if (renderer) renderer.requestDraw();
  syncTempoChip();
  showToast(
    t('toast.tempoApplied', {
      bpm: formatBpm(state.tempo.bpm),
      meter: state.tempo.beatsPerBar,
    }),
    { type: 'success' },
  );
}

function snapFlagTime(seconds) {
  let time = clamp(Number(seconds) || 0, 0, getDuration());
  if (state.tempo.snapToGrid) {
    const beat = beatDuration(state.tempo.bpm);
    time = Math.round(time / beat) * beat;
  }
  return Math.round(time * 1000) / 1000;
}

function setBarStartAt(seconds) {
  if (!state.tempo.enabled) {
    showToast(t('toast.barStartNeedsTempo'), { type: 'warning' });
    return;
  }
  state.tempo.barStartSec = snapFlagTime(seconds);
  state.dirty.project = true;
  if (renderer) renderer.requestDraw();
  showToast(
    t('toast.barStartSet', { time: formatTime(state.tempo.barStartSec, { decimals: 3 }) }),
    { type: 'success' },
  );
}

function clearTempo() {
  state.tempo = defaultTempo();
  state.dirty.project = true;
  if (renderer) renderer.requestDraw();
  syncTempoChip();
  showToast(t('toast.tempoCleared'), { type: 'info' });
}

function toggleGrid() {
  state.tempo.showGrid = !state.tempo.showGrid;
  state.dirty.project = true;
  if (renderer) renderer.requestDraw();
}

function openMasterMenu(x, y, time) {
  if (!menu) return;
  const tempo = state.tempo;
  menu.open({
    x,
    y,
    invoker: dom.laneCanvas,
    header: { label: t('menu.masterHeader') },
    items: [
      { label: t('menu.tempoSettings'), icon: 'icon-metronome', action: openTempo },
      { label: t('menu.setBarStart'), icon: 'icon-flag', action: () => setBarStartAt(time) },
      { separator: true },
      {
        label: tempo.showGrid ? t('menu.hideGrid') : t('menu.showGrid'),
        icon: 'icon-grid',
        disabled: !tempo.enabled,
        action: toggleGrid,
      },
      { separator: true },
      { label: t('menu.fit'), icon: 'icon-fit', action: fitViewNow },
      { label: t('menu.zoomIn'), icon: 'icon-zoom-in', action: () => zoomAt(1.25, time) },
      { label: t('menu.zoomOut'), icon: 'icon-zoom-out', action: () => zoomAt(0.8, time) },
      { separator: true },
      {
        label: t('menu.clearTempo'),
        disabled: !tempo.enabled,
        action: clearTempo,
      },
    ],
  });
}

function openBandMenu(x, y, bandIndex, time) {
  if (!menu) return;
  const band = state.bands[bandIndex];
  if (!band) return;
  const range = formatHzRange(band.lowHz, band.highHz);
  const csvDisabled = !state.tempo.enabled || !state.analysis || state.exporting;
  let footer = null;
  if (!state.tempo.enabled) footer = t('menu.csvDisabled');
  else if (!state.analysis) footer = t('toast.noAudio');
  menu.open({
    x,
    y,
    invoker: dom.laneCanvas,
    header: {
      label: bandLabel(band, bandIndex),
      meta: range,
      color: band.color,
    },
    items: [
      {
        label: state.exporting ? t('menu.exporting') : t('menu.exportCsv'),
        icon: 'icon-download',
        disabled: csvDisabled,
        action: () => exportBand(bandIndex),
      },
      { separator: true },
      {
        label: t('menu.bandSettings'),
        icon: 'icon-sliders',
        action: () => openSettings('analysis'),
      },
      { label: t('menu.fit'), icon: 'icon-fit', action: fitViewNow },
      { label: t('menu.zoomIn'), icon: 'icon-zoom-in', action: () => zoomAt(1.25, time) },
      { label: t('menu.zoomOut'), icon: 'icon-zoom-out', action: () => zoomAt(0.8, time) },
    ],
    footer,
  });
}

function openProjectMenu() {
  if (!menu) return;
  const canSave = Boolean(state.analysis || state.projectAudio);
  menu.open({
    anchor: dom.projectBtn.getBoundingClientRect(),
    align: 'end',
    invoker: dom.projectBtn,
    items: [
      {
        label: t('project.save'),
        icon: 'icon-save',
        disabled: !canSave,
        action: saveProject,
      },
      { label: t('project.load'), icon: 'icon-folder', action: loadProject },
    ],
    footer: canSave ? null : t('project.needAudio'),
  });
}

function openSettings(tab = 'general') {
  if (!settingsDialog) return;
  settingsDialog.open(state.settings, state.appInfo, { tab });
}

function openHelp() {
  if (!dialogs) return;
  dialogs.open(dom.helpDialog, { initialFocus: '#help-close' });
}

/* ------------------------------------------------------------------ */
/* settings handlers                                                   */
/* ------------------------------------------------------------------ */

function handleSettingsChange(patch = {}) {
  if (!patch || typeof patch !== 'object') return;
  Object.assign(state.settings, patch);
  state.settings = normalizeSettings(state.settings);
  const keys = Object.keys(patch);
  if (keys.some((key) => key === 'lang' || key === 'theme' || key === 'scale')) {
    applyPreferences(state.settings.lang, state.settings.theme, state.settings.scale, {
      rebuildHeaders: keys.includes('lang'),
    });
  }
  if (keys.includes('followPlayhead')) {
    state.view.followPlayhead = state.settings.followPlayhead;
    if (state.playback.playing) ensurePlayheadVisible(true);
  }
  if (keys.includes('volume')) {
    state.playback.volume = state.settings.volume;
    syncPlaybackUI();
    if (api) {
      api.invoke('set_volume', { volume: state.playback.volume }).catch((error) => {
        if (error && error.code === 'ERR_AUDIO_DEVICE') {
          showToast(t('toast.audioDevice'), { type: 'warning', timeout: 7000 });
        }
      });
    }
  }
  if (keys.includes('waveformGain') && renderer) {
    renderer.invalidateCache();
    renderer.requestDraw();
  }
  scheduleSaveSettings();
}

async function handleBandsChange({ bandCount, crossovers } = {}) {
  if (!api) return;
  const count = Math.round(clamp(Number(bandCount) || 4, 1, 4));
  const values = Array.isArray(crossovers) ? crossovers.slice(0, Math.max(0, count - 1)) : [];
  const nextBands = deriveBands(values, count);
  const previous = {
    bands: stripBands(state.settings.bands),
    fullBands: state.bands,
    bandCount: state.settings.bandCount,
    crossovers: [...state.crossovers],
    selectedBand: state.selectedBand,
  };

  state.settings.bandCount = count;
  state.settings.bands = nextBands;
  state.crossovers = values;
  state.bands = nextBands.map((band, index) => ({
    ...band,
    index,
    label: bandLabel(band, index),
  }));
  state.selectedBand = clamp(state.selectedBand, 0, count - 1);
  state.view.selectedBand = state.selectedBand;

  try {
    if (state.analysis) {
      const result = await api.invoke('set_bands', {
        bands: nextBands,
        fftSize: state.settings.fftSize,
      });
      const bands = result && Array.isArray(result.bands) && result.bands.length
        ? result.bands
        : nextBands;
      state.analysis = { ...state.analysis, bands };
      state.bands = bands.map((band, index) => ({
        ...band,
        index,
        label: bandLabel(band, index),
      }));
      state.settings.bands = stripBands(state.bands);
    }
    if (renderer) {
      renderer.invalidateData();
      renderer.requestDraw();
    }
    refreshLaneHeaders();
    relayout();
    syncAll();
    scheduleSaveSettings();
    showToast(t('toast.bandsUpdated'), { type: 'success' });
  } catch (error) {
    state.settings.bands = previous.bands;
    state.settings.bandCount = previous.bandCount;
    state.crossovers = previous.crossovers;
    state.bands = previous.fullBands;
    state.selectedBand = previous.selectedBand;
    state.view.selectedBand = previous.selectedBand;
    if (settingsDialog) settingsDialog.sync(state.settings);
    if (renderer) {
      renderer.invalidateData();
      renderer.requestDraw();
    }
    refreshLaneHeaders();
    relayout();
    syncAll();
    errorToast(error, 'toast.bandsFailed');
  }
}

async function handleFftRequest(size, revert) {
  const nextSize = Number(size);
  if (!state.analysis) {
    state.settings.fftSize = nextSize;
    if (settingsDialog) settingsDialog.setFft(nextSize);
    scheduleSaveSettings();
    return;
  }
  let accepted = false;
  try {
    accepted = await confirmDialog.ask({
      title: t('settings.confirmFftTitle'),
      message: t('settings.confirmFftBody', { size: nextSize }),
      confirmLabel: t('settings.confirm'),
      cancelLabel: t('settings.cancel'),
    });
  } catch {
    accepted = false;
  }
  if (!accepted) {
    if (typeof revert === 'function') revert();
    return;
  }

  const previousSize = state.settings.fftSize;
  state.settings.fftSize = nextSize;
  if (settingsDialog) settingsDialog.setFft(nextSize);
  await saveSettingsNow();
  const position = state.playback.positionSec;
  showLoading(true);
  try {
    const result = await api.invoke('reanalyze', {
      path: state.meta?.path,
      fftSize: nextSize,
    });
    applyAnalysis(result, {
      resetTempo: false,
      resetView: false,
      position,
      preserveSelection: true,
    });
    showToast(t('toast.reanalyzed', { size: nextSize }), { type: 'success' });
    scheduleSaveSettings();
  } catch (error) {
    state.settings.fftSize = previousSize;
    if (settingsDialog) settingsDialog.setFft(previousSize);
    scheduleSaveSettings();
    errorToast(error, 'toast.reanalyzeFailed');
  } finally {
    showLoading(false);
  }
}

/* ------------------------------------------------------------------ */
/* dialogs and lane selection                                          */
/* ------------------------------------------------------------------ */

function setupDialogs() {
  dialogs = new DialogController();
  tempoDialog = new TempoDialog(dom.tempoDialog, dialogs, {
    onApply: applyTempo,
    getDuration,
    getPlayhead: () => state.playback.positionSec,
  });
  settingsDialog = new SettingsDialog(dom.settingsDialog, dialogs, {
    onChange: handleSettingsChange,
    onBandsChange: handleBandsChange,
    onFftRequest: handleFftRequest,
    onClose: () => saveSettingsNow(),
  });
  confirmDialog = new ConfirmDialog(dom.confirmDialog, dialogs);
  dialogs.bind(dom.helpDialog);
}

function selectLane(lane, index = 0) {
  if (lane === 'band') {
    const next = Math.round(clamp(Number(index) || 0, 0, Math.max(0, state.bands.length - 1)));
    state.selectedLane = 'band';
    state.selectedBand = next;
    state.view.selectedBand = next;
  } else {
    state.selectedLane = 'master';
  }
  state.dirty.project = true;
  updateLaneHeaderSelection();
  if (renderer) renderer.requestDraw();
  syncStatus();
}

function cacheDom() {
  dom.app = document.getElementById('app');
  dom.fileChip = document.getElementById('file-chip');
  dom.fileName = document.getElementById('file-name');
  dom.fileDuration = document.getElementById('file-duration');
  dom.openBtn = document.getElementById('open-btn');
  dom.projectBtn = document.getElementById('project-btn');
  dom.settingsBtn = document.getElementById('settings-btn');
  dom.helpBtn = document.getElementById('help-btn');
  dom.missingBanner = document.getElementById('missing-banner');
  dom.missingPath = document.getElementById('missing-path');
  dom.relinkBtn = document.getElementById('relink-btn');
  dom.playBtn = document.getElementById('play-btn');
  dom.playIcon = document.getElementById('play-icon');
  dom.playUse = dom.playIcon ? dom.playIcon.querySelector('use') : null;
  dom.playLabel = document.getElementById('play-label');
  dom.stopBtn = document.getElementById('stop-btn');
  dom.timeCurrent = document.getElementById('time-current');
  dom.timeTotal = document.getElementById('time-total');
  dom.seek = document.getElementById('seek');
  dom.volumeBtn = document.getElementById('volume-btn');
  dom.volumeIcon = document.getElementById('volume-icon');
  dom.volumeUse = dom.volumeIcon ? dom.volumeIcon.querySelector('use') : null;
  dom.volume = document.getElementById('volume');
  dom.tempoChip = document.getElementById('tempo-chip');
  dom.tempoChipText = document.getElementById('tempo-chip-text');
  dom.rulerCanvas = document.getElementById('ruler-canvas');
  dom.laneCanvas = document.getElementById('lane-canvas');
  dom.laneView = document.getElementById('lane-view');
  dom.laneHeaders = document.getElementById('lane-headers');
  dom.emptyState = document.getElementById('empty-state');
  dom.emptyOpenBtn = document.getElementById('empty-open-btn');
  dom.loadingOverlay = document.getElementById('loading-overlay');
  dom.loadingPercent = document.getElementById('loading-percent');
  dom.loadingBar = document.getElementById('loading-bar');
  dom.loadingPhases = [...document.querySelectorAll('.loading-phase')];
  dom.hscroll = document.getElementById('hscroll');
  dom.hscrollThumb = document.getElementById('hscroll-thumb');
  dom.statusSr = document.getElementById('status-sr');
  dom.statusCh = document.getElementById('status-ch');
  dom.statusDuration = document.getElementById('status-duration');
  dom.statusAnalysis = document.getElementById('status-analysis');
  dom.statusZoom = document.getElementById('status-zoom');
  dom.statusLane = document.getElementById('status-lane');
  dom.toastRegion = document.getElementById('toast-region');
  dom.menuRoot = document.getElementById('menu-root');
  dom.tempoDialog = document.getElementById('tempo-dialog');
  dom.settingsDialog = document.getElementById('settings-dialog');
  dom.helpDialog = document.getElementById('help-dialog');
  dom.confirmDialog = document.getElementById('confirm-dialog');
  dom.helpClose = document.getElementById('help-close');
  dom.helpClose2 = document.getElementById('help-close-2');
}

/* ------------------------------------------------------------------ */
/* event wiring                                                        */
/* ------------------------------------------------------------------ */

function pointIn(element, event) {
  const rect = element.getBoundingClientRect();
  return {
    x: clamp(event.clientX - rect.left, 0, rect.width),
    y: clamp(event.clientY - rect.top, 0, rect.height),
  };
}

function startFlagDrag(event, canvas) {
  if (!renderer || !state.tempo.enabled) return false;
  flagDrag = { pointerId: event.pointerId, canvas };
  try {
    canvas.setPointerCapture(event.pointerId);
  } catch {
    /* pointer capture is best effort */
  }
  canvas.style.cursor = 'col-resize';
  updateFlagDrag(event, canvas);
  event.preventDefault();
  return true;
}

function updateFlagDrag(event, canvas) {
  if (!flagDrag || !renderer) return;
  const { x } = pointIn(canvas, event);
  state.tempo.barStartSec = snapFlagTime(renderer.timeAtX(x));
  renderer.requestDraw();
}

function endFlagDrag(event) {
  if (!flagDrag) return;
  if (event && event.pointerId !== flagDrag.pointerId) return;
  const canvas = flagDrag.canvas;
  try {
    canvas.releasePointerCapture(flagDrag.pointerId);
  } catch {
    /* already released */
  }
  flagDrag = null;
  canvas.style.cursor = 'default';
  state.dirty.project = true;
}

function setupTransport() {
  dom.playBtn.addEventListener('click', togglePlay);
  dom.stopBtn.addEventListener('click', stopPlayback);
  dom.tempoChip.addEventListener('click', openTempo);
  dom.volumeBtn.addEventListener('click', toggleMute);

  dom.seek.addEventListener('pointerdown', () => {
    seekDragging = true;
  });
  dom.seek.addEventListener('input', () => {
    seekDragging = true;
    seekTo(Number(dom.seek.value), { commit: true });
  });
  dom.seek.addEventListener('change', () => {
    seekDragging = false;
    seekTo(Number(dom.seek.value));
  });
  dom.seek.addEventListener('blur', () => {
    seekDragging = false;
  });

  dom.volume.addEventListener('input', () => {
    setVolume(Number(dom.volume.value), { commit: false });
  });
  dom.volume.addEventListener('change', () => {
    setVolume(Number(dom.volume.value));
  });

  window.addEventListener('pointerup', () => {
    if (!seekDragging) return;
    seekDragging = false;
    seekTo(Number(dom.seek.value));
  });
}

function setupHeader() {
  dom.openBtn.addEventListener('click', openAudio);
  dom.emptyOpenBtn.addEventListener('click', openAudio);
  dom.relinkBtn.addEventListener('click', relinkAudio);
  dom.settingsBtn.addEventListener('click', () => openSettings('general'));
  dom.helpBtn.addEventListener('click', openHelp);
  dom.projectBtn.addEventListener('click', openProjectMenu);
  if (dom.helpClose) {
    dom.helpClose.addEventListener('click', () => {
      if (dialogs) dialogs.close(dom.helpDialog);
    });
  }
  if (dom.helpClose2) {
    dom.helpClose2.addEventListener('click', () => {
      if (dialogs) dialogs.close(dom.helpDialog);
    });
  }
}

function setupCanvasEvents() {
  dom.laneCanvas.addEventListener('pointerdown', (event) => {
    if (event.button !== 0 || !renderer) return;
    const { x, y } = pointIn(dom.laneCanvas, event);
    if (state.tempo.enabled && renderer.isNearFlag(x)) {
      startFlagDrag(event, dom.laneCanvas);
      return;
    }
    const lane = renderer.laneAtY(y);
    if (lane == null) return;
    selectLane(lane === 0 ? 'master' : 'band', lane - 1);
    seekTo(renderer.timeAtX(x));
  });

  dom.laneCanvas.addEventListener('pointermove', (event) => {
    if (!renderer) return;
    if (flagDrag) {
      updateFlagDrag(event, dom.laneCanvas);
      return;
    }
    const { x } = pointIn(dom.laneCanvas, event);
    const duration = getDuration();
    renderer.hoverTime = duration > 0 ? clamp(renderer.timeAtX(x), 0, duration) : null;
    dom.laneCanvas.style.cursor = state.tempo.enabled && renderer.isNearFlag(x)
      ? 'col-resize'
      : 'default';
    renderer.requestDraw();
  });

  dom.laneCanvas.addEventListener('pointerup', (event) => endFlagDrag(event));
  dom.laneCanvas.addEventListener('pointercancel', (event) => endFlagDrag(event));
  dom.laneCanvas.addEventListener('pointerleave', () => {
    if (!renderer || flagDrag) return;
    renderer.hoverTime = null;
    dom.laneCanvas.style.cursor = 'default';
    renderer.requestDraw();
  });

  dom.laneView.addEventListener('contextmenu', (event) => {
    if (!renderer) return;
    event.preventDefault();
    const { x, y } = pointIn(dom.laneView, event);
    const lane = renderer.laneAtY(y) ?? 0;
    if (lane === 0) selectLane('master');
    else selectLane('band', lane - 1);
    const time = clamp(renderer.timeAtX(x), 0, getDuration());
    if (lane === 0) openMasterMenu(event.clientX, event.clientY, time);
    else openBandMenu(event.clientX, event.clientY, lane - 1, time);
  });

  dom.laneView.addEventListener('wheel', (event) => onWheel(event, dom.laneView), { passive: false });

  dom.rulerCanvas.addEventListener('pointerdown', (event) => {
    if (event.button !== 0 || !renderer) return;
    const { x } = pointIn(dom.rulerCanvas, event);
    if (state.tempo.enabled && renderer.isNearFlag(x)) {
      startFlagDrag(event, dom.rulerCanvas);
      return;
    }
    rulerDrag = true;
    try {
      dom.rulerCanvas.setPointerCapture(event.pointerId);
    } catch {
      /* pointer capture is best effort */
    }
    seekTo(renderer.timeAtX(x));
    renderer.hoverTime = getDuration() > 0 ? clamp(renderer.timeAtX(x), 0, getDuration()) : null;
    renderer.requestDraw();
  });

  dom.rulerCanvas.addEventListener('pointermove', (event) => {
    if (!renderer) return;
    if (flagDrag) {
      updateFlagDrag(event, dom.rulerCanvas);
      return;
    }
    const { x } = pointIn(dom.rulerCanvas, event);
    if (rulerDrag) seekTo(renderer.timeAtX(x));
    const duration = getDuration();
    renderer.hoverTime = duration > 0 ? clamp(renderer.timeAtX(x), 0, duration) : null;
    dom.rulerCanvas.style.cursor = state.tempo.enabled && renderer.isNearFlag(x)
      ? 'col-resize'
      : 'default';
    renderer.requestDraw();
  });

  const finishRuler = (event) => {
    if (flagDrag) {
      endFlagDrag(event);
      return;
    }
    if (rulerDrag) {
      rulerDrag = false;
      try {
        dom.rulerCanvas.releasePointerCapture(event.pointerId);
      } catch {
        /* already released */
      }
    }
  };
  dom.rulerCanvas.addEventListener('pointerup', finishRuler);
  dom.rulerCanvas.addEventListener('pointercancel', finishRuler);
  dom.rulerCanvas.addEventListener('pointerleave', () => {
    if (!renderer || rulerDrag || flagDrag) return;
    renderer.hoverTime = null;
    dom.rulerCanvas.style.cursor = 'default';
    renderer.requestDraw();
  });
  dom.rulerCanvas.addEventListener('wheel', (event) => onWheel(event, dom.rulerCanvas), { passive: false });
}

function onWheel(event, element) {
  if (!renderer) return;
  if (event.ctrlKey || event.metaKey) {
    event.preventDefault();
    if (!(getDuration() > 0)) return;
    const { x } = pointIn(element, event);
    const anchor = renderer.timeAtX(x);
    zoomAt(event.deltaY < 0 ? 1.18 : 1 / 1.18, anchor);
    return;
  }
  const max = maxScrollNow();
  if (!(max > 0)) return;
  event.preventDefault();
  const unit = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? renderer.height : 1;
  const delta = (Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY) * unit;
  state.view.scrollSec = clamp(
    state.view.scrollSec + delta / state.view.zoomPxPerSec,
    0,
    max,
  );
  afterViewChange();
}

function setupHscroll() {
  let drag = null;
  const thumb = dom.hscrollThumb;

  thumb.addEventListener('pointerdown', (event) => {
    const max = maxScrollNow();
    if (!(max > 0)) return;
    event.preventDefault();
    drag = {
      startX: event.clientX,
      startScroll: state.view.scrollSec,
      max,
      track: Math.max(1, dom.hscroll.clientWidth),
      thumb: Math.max(1, thumb.getBoundingClientRect().width),
    };
    try {
      thumb.setPointerCapture(event.pointerId);
    } catch {
      /* pointer capture is best effort */
    }
  });

  thumb.addEventListener('pointermove', (event) => {
    if (!drag) return;
    const range = drag.track - drag.thumb;
    if (!(range > 0)) return;
    state.view.scrollSec = clamp(
      drag.startScroll + ((event.clientX - drag.startX) / range) * drag.max,
      0,
      drag.max,
    );
    afterViewChange();
  });

  const finish = (event) => {
    if (!drag) return;
    drag = null;
    try {
      thumb.releasePointerCapture(event.pointerId);
    } catch {
      /* already released */
    }
  };
  thumb.addEventListener('pointerup', finish);
  thumb.addEventListener('pointercancel', finish);

  thumb.addEventListener('keydown', (event) => {
    const max = maxScrollNow();
    if (!(max > 0)) return;
    const width = renderer?.width || 0;
    const visible = width / (Number(state.view.zoomPxPerSec) || 96);
    let next = state.view.scrollSec;
    if (event.key === 'ArrowLeft') next -= visible * 0.1;
    else if (event.key === 'ArrowRight') next += visible * 0.1;
    else if (event.key === 'Home') next = 0;
    else if (event.key === 'End') next = max;
    else return;
    event.preventDefault();
    state.view.scrollSec = clamp(next, 0, max);
    afterViewChange();
  });
}

async function setupNativeDrag() {
  if (!api || api.kind !== 'tauri' || typeof api.onDragDrop !== 'function') return;
  try {
    await api.onDragDrop((event) => {
      const payload = event?.payload || event || {};
      const type = payload.type;
      if (type === 'enter' || type === 'over') {
        dom.laneView.classList.add('drag-over');
      } else if (type === 'leave') {
        dom.laneView.classList.remove('drag-over');
      } else if (type === 'drop') {
        dom.laneView.classList.remove('drag-over');
        const path = Array.isArray(payload.paths) ? payload.paths[0] : null;
        if (path) loadAudio(path);
      }
    });
  } catch {
    /* native drag and drop is optional */
  }
}

function setupBrowserDrag() {
  if (!api || api.kind === 'tauri') return;
  const hasFiles = (event) => {
    const types = event.dataTransfer?.types;
    return types ? Array.from(types).includes('Files') : false;
  };
  let depth = 0;
  document.addEventListener('dragenter', (event) => {
    if (!hasFiles(event)) return;
    event.preventDefault();
    depth += 1;
    dom.laneView.classList.add('drag-over');
  });
  document.addEventListener('dragover', (event) => {
    if (hasFiles(event)) event.preventDefault();
  });
  document.addEventListener('dragleave', (event) => {
    if (!hasFiles(event) && event.relatedTarget !== null) return;
    depth = Math.max(0, depth - 1);
    if (depth === 0 || event.relatedTarget === null) {
      depth = 0;
      dom.laneView.classList.remove('drag-over');
    }
  });
  document.addEventListener('drop', (event) => {
    if (!hasFiles(event)) return;
    event.preventDefault();
    depth = 0;
    dom.laneView.classList.remove('drag-over');
    const file = event.dataTransfer?.files?.[0];
    if (!file) return;
    if (!isAudioFile(file.name)) {
      showToast(t('toast.unsupportedDrop'), { type: 'warning' });
      return;
    }
    loadAudio(file.name);
  });
}

function setupKeyboard() {
  document.addEventListener('keydown', (event) => {
    if (event.defaultPrevented) return;
    if (menu && menu.isOpen) {
      if (event.key === 'Escape') menu.close({ restoreFocus: true });
      return;
    }
    if (document.querySelector('dialog[open]')) return;
    const target = event.target;
    const tag = target && target.tagName ? target.tagName : '';
    if (target && (target.isContentEditable || tag === 'INPUT' || tag === 'SELECT' || tag === 'TEXTAREA')) {
      return;
    }
    if (tag === 'BUTTON' && (event.key === ' ' || event.key === 'Enter')) return;
    if (event.ctrlKey || event.metaKey || event.altKey) return;

    if (event.key === ' ' || event.code === 'Space') {
      if (event.repeat) return;
      event.preventDefault();
      togglePlay();
      return;
    }
    if (event.key === 's' || event.key === 'S') {
      event.preventDefault();
      stopPlayback();
      return;
    }
    if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
      event.preventDefault();
      const delta = (event.shiftKey ? 5 : 1) * (event.key === 'ArrowLeft' ? -1 : 1);
      seekTo(state.playback.positionSec + delta);
      return;
    }
    if (event.key === 'Home' || event.key === 'End') {
      event.preventDefault();
      seekTo(event.key === 'Home' ? 0 : getDuration());
      return;
    }
    if (event.key === '+' || event.key === '=' || event.key === '-' || event.key === '_') {
      event.preventDefault();
      zoomAt(event.key === '-' || event.key === '_' ? 0.8 : 1.25, zoomAnchor());
      return;
    }
    if (event.key === 'Escape' && menu && menu.isOpen) {
      menu.close({ restoreFocus: true });
    }
  });
}

function setupResizeObserver() {
  const schedule = () => {
    if (resizeRaf) return;
    resizeRaf = requestAnimationFrame(() => {
      resizeRaf = 0;
      onResize();
    });
  };
  if (typeof ResizeObserver === 'function') {
    const observer = new ResizeObserver(schedule);
    observer.observe(dom.laneView);
    if (dom.rulerCanvas && dom.rulerCanvas.parentElement) observer.observe(dom.rulerCanvas.parentElement);
  }
  window.addEventListener('resize', schedule);
}

function onResize() {
  if (!renderer) return;
  renderer.resize();
  relayout();
  state.view = clampView(state.view, getDuration(), renderer.width);
  renderer.requestDraw();
  updateHscroll();
  updateRulerAria();
  syncStatus();
}

function bindEvents() {
  setupTransport();
  setupHeader();
  setupCanvasEvents();
  setupHscroll();
}

/* ------------------------------------------------------------------ */
/* boot                                                                */
/* ------------------------------------------------------------------ */

async function setupProgress() {
  if (!api) return;
  try {
    const unlisten = await api.listen('analysis-progress', (event) => {
      setProgress(event?.payload || {});
    });
    if (typeof unlisten !== 'function') return;
  } catch {
    /* progress is a nicety; analysis still completes */
  }
}

async function loadInitialSettings() {
  let loaded = null;
  try {
    loaded = await api.invoke('load_settings');
  } catch {
    settingsLoadFailed = true;
  }
  const base = normalizeSettings(loaded || state.settings);
  const cache = readUiCache();
  if (cache) {
    base.lang = cache.lang;
    base.theme = cache.theme;
    base.scale = cache.scale;
  } else {
    const bootTheme = document.documentElement.dataset.theme;
    if (bootTheme === 'light' || bootTheme === 'dark') base.theme = bootTheme;
  }
  return normalizeSettings(base);
}

async function loadAppInfo() {
  try {
    state.appInfo = await api.invoke('app_info');
  } catch {
    state.appInfo = {
      version: '0.1.0',
      dataDir: '—',
      portable: null,
      webview2: null,
    };
  }
}

async function handleStartup() {
  try {
    const args = await api.invoke('startup_args');
    if (!args) return;
    if (args.projectPath) {
      await loadProjectFromPath(args.projectPath);
    } else if (args.openPath) {
      await loadAudio(args.openPath);
    }
  } catch {
    /* startup arguments are optional */
  }
}

async function init() {
  api = await initApi();
  const settings = await loadInitialSettings();
  state.settings = settings;
  state.bands = settings.bands.map((band, index) => ({
    ...band,
    index,
    label: bandLabel(band, index),
  }));
  state.crossovers = crossoversFromBands(settings.bands);
  state.view.followPlayhead = settings.followPlayhead;
  state.playback.volume = settings.volume;
  state.playback.durationSec = 0;
  lastNonZeroVolume = settings.volume > 0 ? settings.volume : 0.8;

  cacheDom();
  renderer = new TimelineRenderer({
    rulerCanvas: dom.rulerCanvas,
    laneCanvas: dom.laneCanvas,
    state,
  });
  menu = new ContextMenu(dom.menuRoot);
  setupDialogs();
  bindEvents();
  applyPreferences(settings.lang, settings.theme, settings.scale, {
    rebuildHeaders: true,
  });
  setupResizeObserver();
  await setupProgress();
  await loadAppInfo();
  syncAll();

  requestAnimationFrame(() => {
    if (!renderer) return;
    renderer.resize();
    relayout();
    state.view = clampView(state.view, getDuration(), renderer.width);
    renderer.requestDraw();
    syncAll();
  });

  startPlaybackPolling();
  await setupNativeDrag();
  setupBrowserDrag();
  setupKeyboard();

  if (settingsLoadFailed) {
    showToast(t('toast.settingsLoadFailed'), { type: 'warning', timeout: 7000 });
  }
  await handleStartup();
}

init().catch((error) => {
  try {
    const message = error && error.message ? error.message : String(error || 'unknown error');
    showToast(t('error.ERR_UNKNOWN', { message }), { type: 'error', timeout: 0 });
    document.body.dataset.bootError = message;
  } catch {
    /* nothing else can be done before the DOM is ready */
  }
});
