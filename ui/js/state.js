/**
 * Application state and UI preference cache.
 *
 * The whole UI reads/writes one mutable state object (spec §6). Keeping the
 * shape here makes persistence and project round-trips predictable.
 */

import {
  defaultBands,
  defaultSettings,
  defaultTempo,
  defaultView,
  normalizeSettings,
} from './core.js';

export const UI_CACHE_KEY = 'namivision.ui';

export function createAppState() {
  const settings = defaultSettings();
  return {
    appInfo: null,
    meta: null,
    projectAudio: null,
    analysis: null,
    bands: defaultBands(),
    tempo: defaultTempo(),
    view: defaultView(),
    playback: {
      positionSec: 0,
      durationSec: 0,
      playing: false,
      volume: settings.volume,
      ended: false,
    },
    settings,
    selectedBand: 0,
    selectedLane: 'master',
    crossovers: [250, 1000, 4000],
    projectPath: null,
    audioMissing: false,
    missingAudioPath: '',
    loading: false,
    exporting: false,
    dirty: { project: false, settings: false, view: false },
  };
}

export function readUiCache() {
  if (typeof localStorage === 'undefined') return null;
  try {
    const raw = localStorage.getItem(UI_CACHE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== 'object') return null;
    return {
      lang: parsed.lang === 'en' ? 'en' : 'ja',
      theme: parsed.theme === 'light' ? 'light' : 'dark',
      scale: Number.isFinite(Number(parsed.scale)) ? Math.min(1.5, Math.max(0.8, Number(parsed.scale))) : 1,
    };
  } catch {
    return null;
  }
}

export function writeUiCache({ lang, theme, scale }) {
  if (typeof localStorage === 'undefined') return;
  try {
    localStorage.setItem(UI_CACHE_KEY, JSON.stringify({ lang, theme, scale }));
  } catch {
    /* private mode / quota — preference cache is best effort */
  }
}

export function settingsFromState(state) {
  return normalizeSettings(state.settings);
}
