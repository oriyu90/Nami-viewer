import assert from 'node:assert/strict';
import test from 'node:test';

import {
  BEAT_TOLERANCE,
  DEFAULT_ZOOM,
  ZOOM_MAX,
  ZOOM_MIN,
  buildBeatCsv,
  chooseTickStep,
  clampView,
  computeBeatDeviations,
  defaultBands,
  defaultSettings,
  defaultTempo,
  deriveBands,
  fitView,
  formatAnalysisMs,
  formatBpm,
  formatHzRange,
  formatSampleRate,
  formatTime,
  generateGrid,
  generateTicks,
  normalizeLoadedProject,
  normalizeProject,
  normalizeSettings,
  normalizeTempo,
  sanitizeCsvFilename,
  sanitizeFilenamePart,
  timeToX,
  visibleRange,
  xToTime,
  zoomViewAt,
} from '../../ui/js/core.js';

/* ------------------------------------------------------------------ */
/* time formatting                                                     */
/* ------------------------------------------------------------------ */

test('formatTime renders mm:ss.cc and pads every field', () => {
  assert.equal(formatTime(0), '00:00.00');
  assert.equal(formatTime(5.5), '00:05.50');
  assert.equal(formatTime(65.5), '01:05.50');
  assert.equal(formatTime(3599.99), '59:59.99');
  assert.equal(formatTime(3661.25), '1:01:01.25');
  assert.equal(formatTime(3661.25, { forceHours: true }), '1:01:01.25');
});

test('formatTime honours decimals 0..3 and rounds carry correctly', () => {
  assert.equal(formatTime(59.996, { decimals: 2 }), '01:00.00');
  assert.equal(formatTime(12.3456, { decimals: 3 }), '00:12.346');
  assert.equal(formatTime(12.6, { decimals: 0 }), '00:13');
  assert.equal(formatTime(1.29, { decimals: 1 }), '00:01.3');
});

test('formatTime is total for negative and non-finite input', () => {
  assert.equal(formatTime(-4), '00:00.00');
  assert.equal(formatTime(Number.NaN), '00:00.00');
  assert.equal(formatTime(Number.POSITIVE_INFINITY), '00:00.00');
  assert.equal(formatTime(undefined), '00:00.00');
});

test('audio metadata formatters stay human readable', () => {
  assert.equal(formatSampleRate(44100), '44.1 kHz');
  assert.equal(formatSampleRate(48000), '48 kHz');
  assert.equal(formatSampleRate(800), '800 Hz');
  assert.equal(formatSampleRate(0), '—');
  assert.equal(formatAnalysisMs(486), '486 ms');
  assert.equal(formatAnalysisMs(1500), '1.5 s');
  assert.equal(formatAnalysisMs(-1), '—');
  assert.equal(formatHzRange(20, 250), '20–250 Hz');
  assert.equal(formatHzRange(250, 1000), '0.25–1 kHz');
  assert.equal(formatBpm(120), '120');
  assert.equal(formatBpm(120.5), '120.5');
});

/* ------------------------------------------------------------------ */
/* ruler ticks                                                         */
/* ------------------------------------------------------------------ */

test('chooseTickStep picks the first step at least minPx wide', () => {
  assert.equal(chooseTickStep(96), 1);
  assert.equal(chooseTickStep(1000), 0.1);
  assert.equal(chooseTickStep(4), 30);
  assert.equal(chooseTickStep(0), 1);
  assert.equal(chooseTickStep(DEFAULT_ZOOM, 200), 5);
});

test('generateTicks aligns to the step and includes both edges', () => {
  assert.deepEqual(generateTicks(0, 2, 0.5), [0, 0.5, 1, 1.5, 2]);
  assert.deepEqual(generateTicks(0.3, 1.1, 0.5), [0.5, 1]);
  assert.deepEqual(generateTicks(0, 1, 0), []);
  assert.deepEqual(generateTicks(2, 1, 0.5), []);
});

/* ------------------------------------------------------------------ */
/* beat grid                                                           */
/* ------------------------------------------------------------------ */

test('generateGrid produces negative beats before a late bar start', () => {
  const tempo = { enabled: true, bpm: 120, beatsPerBar: 4, barStartSec: 1.2 };
  const grid = generateGrid(tempo, 4, 0, 4);
  assert.equal(grid.beatDur, 0.5);
  assert.equal(grid.barDur, 2);
  const byK = new Map(grid.beats.map((beat) => [beat.k, beat]));
  assert.ok(Math.abs(byK.get(-2).time - 0.2) < 1e-9);
  assert.equal(byK.get(-2).bar, 0);
  assert.equal(byK.get(-2).beatInBar, 3);
  assert.ok(Math.abs(byK.get(-1).time - 0.7) < 1e-9);
  assert.equal(byK.get(-1).bar, 0);
  assert.equal(byK.get(-1).beatInBar, 4);
  assert.equal(byK.get(0).time, 1.2);
  assert.equal(byK.get(0).bar, 1);
  assert.equal(byK.get(0).beatInBar, 1);
  assert.equal(byK.get(0).isBar, true);
  assert.equal(byK.get(1).bar, 1);
  assert.equal(byK.get(4).bar, 2);
});

test('generateGrid respects 3/4 bars, subdivisions and disabled state', () => {
  const tempo = {
    enabled: true,
    bpm: 90,
    beatsPerBar: 3,
    barStartSec: 0,
    subdivision: 2,
  };
  const grid = generateGrid(tempo, 4, 0, 4);
  assert.equal(grid.beatDur, 60 / 90);
  assert.ok(grid.bars.length >= 2);
  assert.equal(grid.bars[1].bar, 2);
  assert.equal(grid.bars[1].beatInBar, 1);
  assert.ok(grid.subdivisions.length > 0);
  assert.equal(generateGrid({ ...tempo, enabled: false }, 4).beats.length, 0);
  assert.equal(generateGrid(tempo, 0).beats.length, 0);
});

test('generateGrid clips to the requested view window', () => {
  const tempo = { enabled: true, bpm: 120, beatsPerBar: 4, barStartSec: 0 };
  const grid = generateGrid(tempo, 10, 3, 4);
  assert.ok(grid.beats.length >= 2);
  for (const beat of grid.beats) {
    assert.ok(beat.time >= 3 - 1e-9 && beat.time <= 4 + 1e-9);
  }
});

/* ------------------------------------------------------------------ */
/* view transforms                                                     */
/* ------------------------------------------------------------------ */

test('timeToX and xToTime are inverse transforms', () => {
  const view = { zoomPxPerSec: 80, scrollSec: 2.5 };
  assert.equal(timeToX(3.5, view), 80);
  assert.equal(xToTime(80, view), 3.5);
  assert.equal(timeToX(0, view), -200);
  assert.equal(xToTime(0, view), 2.5);
});

test('clampView limits zoom, scroll and selected band', () => {
  const clamped = clampView(
    { zoomPxPerSec: 99999, scrollSec: 99, selectedBand: 9, followPlayhead: false },
    10,
    500,
  );
  assert.equal(clamped.zoomPxPerSec, ZOOM_MAX);
  assert.equal(clamped.scrollSec, 10 - 500 / ZOOM_MAX);
  assert.equal(clamped.selectedBand, 3);
  assert.equal(clamped.followPlayhead, false);

  const zoomedOut = clampView({ zoomPxPerSec: 0.1, scrollSec: 0 }, 10, 500);
  assert.equal(zoomedOut.zoomPxPerSec, ZOOM_MIN);

  const scrolled = clampView({ zoomPxPerSec: 100, scrollSec: 99 }, 10, 500);
  assert.equal(scrolled.scrollSec, 5);
});

test('zoomViewAt keeps the anchor time under the same pixel', () => {
  const view = { zoomPxPerSec: 50, scrollSec: 10 };
  const anchor = 20;
  const next = zoomViewAt(view, anchor, 100, 100, 400);
  assert.equal(next.zoomPxPerSec, 100);
  const before = timeToX(anchor, view);
  const after = timeToX(anchor, next);
  assert.ok(Math.abs(before - after) < 1e-9);
});

test('fitView fits the duration and visibleRange reports the window', () => {
  const view = fitView(10, 500, { padding: 20 });
  assert.equal(view.scrollSec, 0);
  assert.ok(Math.abs(view.zoomPxPerSec - 48) < 1e-9);
  const range = visibleRange(view, 500);
  assert.equal(range.startSec, 0);
  assert.ok(Math.abs(range.endSec - 500 / view.zoomPxPerSec) < 1e-9);
});

/* ------------------------------------------------------------------ */
/* CSV filenames                                                       */
/* ------------------------------------------------------------------ */

test('sanitizeFilenamePart strips unsafe characters', () => {
  assert.equal(sanitizeFilenamePart('My Song!'), 'My_Song');
  assert.equal(sanitizeFilenamePart('  leading and trailing  '), 'leading_and_trailing');
  assert.equal(sanitizeFilenamePart('日本語'), 'audio');
  assert.equal(sanitizeFilenamePart(''), 'audio');
  assert.equal(sanitizeFilenamePart(null), 'audio');
  assert.equal(sanitizeFilenamePart('a___b'), 'a_b');
});

test('sanitizeCsvFilename includes stem, 1-based band and label', () => {
  assert.equal(
    sanitizeCsvFilename('demo_click_120bpm.wav', 0, 'Low'),
    'demo_click_120bpm_wav_band1_Low_beats.csv',
  );
  assert.equal(
    sanitizeCsvFilename('My Song', 2, 'Mid High'),
    'My_Song_band3_Mid_High_beats.csv',
  );
  assert.equal(
    sanitizeCsvFilename('日本語', -4, '低域'),
    'audio_band1_band_beats.csv',
  );
});

/* ------------------------------------------------------------------ */
/* settings / tempo / project defaults                                 */
/* ------------------------------------------------------------------ */

test('defaultSettings and defaultTempo match the contract', () => {
  const settings = defaultSettings();
  assert.equal(settings.lang, 'ja');
  assert.equal(settings.theme, 'dark');
  assert.equal(settings.scale, 1);
  assert.equal(settings.volume, 0.8);
  assert.equal(settings.fftSize, 2048);
  assert.equal(settings.bandCount, 4);
  assert.equal(settings.csvDelimiter, ',');
  assert.equal(settings.csvDecimals, 3);
  assert.equal(settings.includeUndetected, true);
  assert.deepEqual(defaultBands().map((band) => [band.lowHz, band.highHz]), [
    [20, 250],
    [250, 1000],
    [1000, 4000],
    [4000, 16000],
  ]);

  const tempo = defaultTempo();
  assert.equal(tempo.enabled, false);
  assert.equal(tempo.bpm, 120);
  assert.equal(tempo.beatsPerBar, 4);
  assert.equal(tempo.barStartSec, 0);
  assert.equal(tempo.subdivision, 1);
});

test('normalizeSettings clamps values and rebuilds bands from crossovers', () => {
  const settings = normalizeSettings({
    lang: 'en',
    theme: 'light',
    scale: 9,
    volume: 4,
    waveformGain: 0.01,
    fftSize: 1234,
    bandCount: 3,
    bands: [
      { lowHz: 20, highHz: 300 },
      { lowHz: 300, highHz: 2000 },
      { lowHz: 2000, highHz: 18000 },
    ],
    csvDelimiter: ';',
    csvDecimals: 99,
  });
  assert.equal(settings.lang, 'en');
  assert.equal(settings.theme, 'light');
  assert.equal(settings.scale, 1.5);
  assert.equal(settings.volume, 1);
  assert.equal(settings.waveformGain, 0.25);
  assert.equal(settings.fftSize, 2048);
  assert.equal(settings.bandCount, 3);
  assert.equal(settings.bands.length, 3);
  assert.equal(settings.csvDelimiter, ';');
  assert.equal(settings.csvDecimals, 6);

  const fallback = normalizeSettings({ lang: 'fr', theme: 'neon', scale: 0.1, fftSize: 999 });
  assert.equal(fallback.lang, 'ja');
  assert.equal(fallback.theme, 'dark');
  assert.equal(fallback.scale, 0.8);
  assert.equal(fallback.fftSize, 2048);
});

test('deriveBands maps crossovers onto the 20 Hz..20 kHz range', () => {
  const bands = deriveBands([100, 500, 5000], 4);
  assert.deepEqual(bands.map((band) => [band.lowHz, band.highHz]), [
    [20, 100],
    [100, 500],
    [500, 5000],
    [5000, 20000],
  ]);
  const single = deriveBands([], 1);
  assert.deepEqual(single.map((band) => [band.lowHz, band.highHz]), [[20, 20000]]);
});

test('normalizeTempo rejects invalid meters and clamps bpm', () => {
  const tempo = normalizeTempo({
    enabled: true,
    bpm: 999,
    beatsPerBar: 5,
    barStartSec: -2,
    subdivision: 3,
  });
  assert.equal(tempo.bpm, 400);
  assert.equal(tempo.beatsPerBar, 4);
  assert.equal(tempo.barStartSec, 0);
  assert.equal(tempo.subdivision, 1);
});

test('normalizeProject applies defaults and rejects invalid files', () => {
  const project = normalizeProject({});
  assert.equal(project.schema, 'namivision.project');
  assert.equal(project.version, 1);
  assert.equal(project.audio.path, '');
  assert.equal(project.audio.fileName, '');
  assert.equal(project.bands.length, 4);
  assert.equal(project.playback.volume, 0.8);
  assert.equal(project.ui.lang, 'ja');

  assert.throws(() => normalizeProject(null), (error) => error.code === 'ERR_PROJECT_FORMAT');
  assert.throws(
    () => normalizeProject({ schema: 'other' }),
    (error) => error.code === 'ERR_PROJECT_FORMAT',
  );
  assert.throws(
    () => normalizeProject({ schema: 'namivision.project', version: 2 }),
    (error) => error.code === 'ERR_PROJECT_VERSION' && error.params.version === '2',
  );
});

test('normalizeLoadedProject accepts a wrapper and merges audioMissing', () => {
  const wrapped = normalizeLoadedProject({
    project: {
      audio: { path: 'C:\\Music\\a.wav', fileName: 'a.wav', durationSec: 12 },
      tempo: { enabled: true, bpm: 100, beatsPerBar: 3, barStartSec: 0.5 },
      bands: [
        { lowHz: 20, highHz: 200, color: '#112233' },
        { lowHz: 200, highHz: 20000, color: '#445566' },
      ],
      playback: { positionSec: 3, volume: 0.4 },
      ui: { lang: 'en', theme: 'light', scale: 1.25 },
    },
    audioMissing: true,
  });
  assert.equal(wrapped.audioMissing, true);
  assert.equal(wrapped.audio.fileName, 'a.wav');
  assert.equal(wrapped.tempo.beatsPerBar, 3);
  assert.equal(wrapped.bands.length, 2);
  assert.equal(wrapped.playback.positionSec, 3);
  assert.equal(wrapped.ui.lang, 'en');
  assert.equal(wrapped.ui.scale, 1.25);

  const bare = normalizeLoadedProject({ audio: { path: 'x.wav' } });
  assert.equal(bare.audio.fileName, 'x.wav');
  assert.equal(bare.audioMissing, false);
});

/* ------------------------------------------------------------------ */
/* beat deviations / CSV                                               */
/* ------------------------------------------------------------------ */

function syntheticAnalysis() {
  const hop = 0.01;
  const duration = 4;
  const frames = Math.round(duration / hop) + 1;
  const onset = new Array(frames).fill(0);
  for (const time of [0.5, 1.0, 1.5, 2.0]) {
    const frame = Math.round(time / hop);
    onset[frame] = 0.9;
    onset[frame - 1] = 0.2;
    onset[frame + 1] = 0.2;
  }
  return {
    meta: { durationSec: duration, fileName: 'demo_click_120bpm.wav' },
    frameHopSec: hop,
    bands: [
      {
        index: 0,
        label: 'Low',
        lowHz: 20,
        highHz: 250,
        onset,
      },
    ],
  };
}

test('computeBeatDeviations detects onsets and leaves silent beats undetected', () => {
  const analysis = syntheticAnalysis();
  const rows = computeBeatDeviations({
    analysis,
    tempo: { enabled: true, bpm: 120, beatsPerBar: 4, barStartSec: 0 },
    sensitivity: 0.5,
    tolerance: BEAT_TOLERANCE,
  });
  assert.ok(rows.length >= 8);
  const detected = rows.filter((row) => row.status === 'detected');
  assert.ok(detected.length >= 4);
  for (const row of detected) {
    assert.ok(Math.abs(row.deviationMs) < 15);
    assert.ok(row.strength > 0);
  }
  const silent = rows.find((row) => row.expectedTimeSec === 3);
  assert.ok(silent);
  assert.equal(silent.status, 'undetected');
  assert.equal(silent.onsetTimeSec, null);
});

test('buildBeatCsv emits BOM, CRLF and blank fields for undetected rows', () => {
  const analysis = syntheticAnalysis();
  const result = buildBeatCsv({
    analysis,
    bandIndex: 0,
    tempo: { enabled: true, bpm: 120, beatsPerBar: 4, barStartSec: 0 },
    includeUndetected: true,
    decimals: 3,
  });
  assert.ok(result.csv.startsWith('\uFEFF'));
  assert.ok(result.csv.includes('\r\n'));
  assert.ok(result.csv.includes('band_index,band_label,frequency_range_hz'));
  assert.ok(result.rows > 0);
  assert.ok(result.detected > 0);
  const undetected = result.csv
    .split('\r\n')
    .find((line) => line.endsWith(',undetected'));
  assert.ok(undetected);
  const fields = undetected.split(',');
  assert.equal(fields.length, 12);
  assert.deepEqual(fields.slice(7, 11), ['', '', '', '']);

  const detectedOnly = buildBeatCsv({
    analysis,
    bandIndex: 0,
    tempo: { enabled: true, bpm: 120, beatsPerBar: 4, barStartSec: 0 },
    includeUndetected: false,
  });
  assert.equal(detectedOnly.rows, detectedOnly.detected);
});

test('buildBeatCsv supports semicolon delimiters and returns empty for missing bands', () => {
  const analysis = syntheticAnalysis();
  const result = buildBeatCsv({
    analysis,
    bandIndex: 0,
    tempo: { enabled: true, bpm: 120, beatsPerBar: 4, barStartSec: 0 },
    delimiter: ';',
    decimals: 2,
  });
  assert.ok(result.csv.split('\r\n')[0].includes('band_index;band_label'));
  assert.deepEqual(buildBeatCsv({ analysis, bandIndex: 5, tempo: {} }), {
    csv: '',
    rows: 0,
    detected: 0,
  });
});
