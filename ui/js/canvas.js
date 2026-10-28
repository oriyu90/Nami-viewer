/**
 * Timeline renderer: ruler + master waveform + band envelopes.
 *
 * All geometry is in CSS pixels; the backing store is scaled by DPR.
 * Peak/envelope aggregation is cached per (data, view, width) so scrolling
 * and playback redraws do not re-aggregate the whole file every frame.
 */

import {
  chooseTickStep,
  clamp,
  euclideanMod,
  formatTime,
  generateGrid,
  generateTicks,
  timeToX,
} from './core.js';

const MONO = '"Cascadia Mono", Consolas, ui-monospace, monospace';

function hexToRgba(hex, alpha) {
  if (typeof hex === 'string' && /^#[0-9a-fA-F]{6}$/.test(hex)) {
    const value = Number.parseInt(hex.slice(1), 16);
    const r = (value >> 16) & 255;
    const g = (value >> 8) & 255;
    const b = value & 255;
    return `rgba(${r}, ${g}, ${b}, ${alpha})`;
  }
  return `rgba(147, 161, 179, ${alpha})`;
}

function roundedRect(ctx, x, y, w, h, r) {
  const radius = Math.max(0, Math.min(r, w / 2, h / 2));
  ctx.beginPath();
  ctx.moveTo(x + radius, y);
  ctx.arcTo(x + w, y, x + w, y + h, radius);
  ctx.arcTo(x + w, y + h, x, y + h, radius);
  ctx.arcTo(x, y + h, x, y, radius);
  ctx.arcTo(x, y, x + w, y, radius);
  ctx.closePath();
}

export class TimelineRenderer {
  constructor({ rulerCanvas, laneCanvas, state }) {
    this.rulerCanvas = rulerCanvas;
    this.laneCanvas = laneCanvas;
    this.rulerCtx = rulerCanvas.getContext('2d');
    this.laneCtx = laneCanvas.getContext('2d');
    this.state = state;
    this.dpr = 1;
    this.width = 0;
    this.height = 0;
    this.rulerW = 0;
    this.rulerH = 0;
    this.laneHeights = [96];
    this.hoverTime = null;
    this.colors = null;
    this.themeKey = '';
    this.dataVersion = 0;
    this.cache = { key: '', master: null, bands: [] };
    this.raf = 0;
    this.scale = 1;
  }

  resize() {
    const dpr = Math.max(1, window.devicePixelRatio || 1);
    const laneW = Math.max(1, Math.round(this.laneCanvas.clientWidth));
    const laneH = Math.max(1, Math.round(this.laneCanvas.clientHeight));
    const rulerW = Math.max(1, Math.round(this.rulerCanvas.clientWidth));
    const rulerH = Math.max(1, Math.round(this.rulerCanvas.clientHeight));
    this.dpr = dpr;
    this.width = laneW;
    this.height = laneH;
    this.rulerW = rulerW;
    this.rulerH = rulerH;
    this.laneCanvas.width = Math.round(laneW * dpr);
    this.laneCanvas.height = Math.round(laneH * dpr);
    this.rulerCanvas.width = Math.round(rulerW * dpr);
    this.rulerCanvas.height = Math.round(rulerH * dpr);
    this.invalidateCache();
    this.requestDraw();
  }

  setLayout(heights) {
    const raw = Array.isArray(heights) ? heights.map((h) => Math.max(1, Math.round(Number(h) || 1))) : [96];
    const sum = raw.reduce((a, b) => a + b, 0);
    if (sum !== this.height && raw.length > 0) {
      raw[raw.length - 1] += this.height - sum;
      if (raw[raw.length - 1] < 1) raw[raw.length - 1] = 1;
    }
    this.laneHeights = raw;
    this.invalidateCache();
    this.requestDraw();
  }

  setScale(scale) {
    this.scale = clamp(scale, 0.8, 1.5);
    this.requestDraw();
  }

  invalidateTheme() {
    this.colors = null;
    this.requestDraw();
  }

  invalidateData() {
    this.dataVersion += 1;
    this.cache = { key: '', master: null, bands: [] };
    this.requestDraw();
  }

  invalidateCache() {
    this.cache = { key: '', master: null, bands: [] };
  }

  requestDraw() {
    if (this.raf) return;
    this.raf = requestAnimationFrame(() => {
      this.raf = 0;
      this.draw();
    });
  }

  getColors() {
    const theme = document.documentElement.dataset.theme || 'dark';
    if (this.colors && this.themeKey === theme) return this.colors;
    const cs = getComputedStyle(document.documentElement);
    const read = (name, fallback) => {
      const value = cs.getPropertyValue(name).trim();
      return value || fallback;
    };
    this.colors = {
      canvas: read('--canvas', '#0e1116'),
      surface: read('--surface', '#151a21'),
      surface2: read('--surface-2', '#1b222b'),
      rule: read('--rule', '#26303c'),
      ruleStrong: read('--rule-strong', '#354354'),
      ink: read('--ink', '#e8edf4'),
      muted: read('--muted', '#93a1b3'),
      action: read('--action', '#ffb74d'),
      onAction: read('--on-action', '#201603'),
      wave: read('--wave', '#e8edf4'),
      gridBar: read('--grid-bar', 'rgba(232,237,244,.26)'),
      gridBeat: read('--grid-beat', 'rgba(232,237,244,.14)'),
      gridSub: read('--grid-sub', 'rgba(232,237,244,.08)'),
      hover: read('--hover-line', 'rgba(232,237,244,.4)'),
      masterBg: read('--lane-master-bg', '#151a21'),
      bandBg: read('--lane-band-bg', '#0e1116'),
      selectedBg: read('--lane-selected-bg', 'rgba(232,237,244,.025)'),
    };
    this.themeKey = theme;
    return this.colors;
  }

  get duration() {
    const value = Number(this.state?.meta?.durationSec ?? this.state?.playback?.durationSec);
    return Number.isFinite(value) && value > 0 ? value : 0;
  }

  get view() {
    const view = this.state?.view || {};
    return {
      zoomPxPerSec: clamp(Number(view.zoomPxPerSec) || 96, 2, 4000),
      scrollSec: Math.max(0, Number(view.scrollSec) || 0),
      selectedBand: Math.round(clamp(Number(view.selectedBand) || 0, 0, 3)),
      followPlayhead: view.followPlayhead !== false,
    };
  }

  bounds() {
    const out = [];
    let top = 0;
    for (const height of this.laneHeights) {
      out.push({ top, height });
      top += height;
    }
    return out;
  }

  laneAtY(y) {
    if (!Number.isFinite(y) || y < 0 || y >= this.height) return null;
    let top = 0;
    for (let i = 0; i < this.laneHeights.length; i += 1) {
      const height = this.laneHeights[i];
      if (y >= top && y < top + height) return i;
      top += height;
    }
    return null;
  }

  timeAtX(x) {
    return this.view.scrollSec + x / this.view.zoomPxPerSec;
  }

  xAtTime(time) {
    return timeToX(time, this.view);
  }

  flagX() {
    const tempo = this.state?.tempo;
    if (!tempo?.enabled) return null;
    return this.xAtTime(Number(tempo.barStartSec) || 0);
  }

  isNearFlag(x) {
    const fx = this.flagX();
    if (fx == null) return false;
    return Math.abs(x - fx) <= Math.max(8, 10 * this.scale);
  }

  draw() {
    if (!this.width || !this.height) return;
    const colors = this.getColors();
    this.drawLanes(colors);
    this.drawRuler(colors);
  }

  /* ------------------------------------------------------------------ */
  /* lanes                                                              */
  /* ------------------------------------------------------------------ */

  drawLanes(colors) {
    const ctx = this.laneCtx;
    const width = this.width;
    const height = this.height;
    const scale = this.scale;
    const view = this.view;
    const duration = this.duration;
    const bounds = this.bounds();
    const state = this.state;
    const tempo = state.tempo || {};

    ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    ctx.clearRect(0, 0, width, height);
    ctx.fillStyle = colors.bandBg;
    ctx.fillRect(0, 0, width, height);

    if (bounds[0]) {
      ctx.fillStyle = colors.masterBg;
      ctx.fillRect(0, 0, width, bounds[0].height);
    }

    // selected lane tint
    const selectedIndex = state.selectedLane === 'master' ? 0 : 1 + (Number(state.selectedBand) || 0);
    if (bounds[selectedIndex]) {
      const b = bounds[selectedIndex];
      ctx.fillStyle = colors.selectedBg;
      ctx.fillRect(0, b.top, width, b.height);
    }

    if (tempo.enabled && tempo.showGrid !== false && duration > 0) {
      this.drawGrid(ctx, colors, view, duration, tempo, bounds, scale);
    }

    if (state.analysis) {
      this.drawMaster(ctx, colors, view, bounds[0], scale);
      const bands = Array.isArray(state.bands) ? state.bands : [];
      for (let i = 0; i < bands.length; i += 1) {
        const bound = bounds[i + 1];
        if (!bound) break;
        this.drawBand(ctx, colors, bands[i], i, bound, view, scale);
      }
    } else {
      for (const b of bounds) {
        this.centerLine(ctx, colors, b.top, b.height, 0.16);
      }
    }

    // lane separators
    ctx.strokeStyle = colors.rule;
    ctx.lineWidth = 1;
    ctx.beginPath();
    let top = 0;
    for (let i = 0; i < bounds.length - 1; i += 1) {
      top += bounds[i].height;
      ctx.moveTo(0, top + 0.5);
      ctx.lineTo(width, top + 0.5);
    }
    ctx.stroke();

    // selected lane left accent
    if (bounds[selectedIndex]) {
      const b = bounds[selectedIndex];
      const band = selectedIndex > 0 ? state.bands?.[selectedIndex - 1] : null;
      ctx.fillStyle = band?.color || colors.wave;
      ctx.fillRect(0, b.top, 2, b.height);
    }

    if (tempo.enabled) {
      const fx = this.flagX();
      if (fx != null && fx >= -2 && fx <= width + 2) {
        ctx.save();
        ctx.strokeStyle = colors.action;
        ctx.globalAlpha = 0.9;
        ctx.lineWidth = 1;
        ctx.setLineDash([4 * scale, 3 * scale]);
        ctx.beginPath();
        ctx.moveTo(Math.round(fx) + 0.5, 0);
        ctx.lineTo(Math.round(fx) + 0.5, height);
        ctx.stroke();
        ctx.restore();
      }
    }

    this.drawPlayhead(ctx, colors, view, width, height, scale);
    this.drawHover(ctx, colors, view, width, height, scale);
  }

  drawGrid(ctx, colors, view, duration, tempo, bounds, scale) {
    const width = this.width;
    const start = view.scrollSec;
    const end = start + width / view.zoomPxPerSec;
    const grid = generateGrid(tempo, duration, start, end, { maxBeats: 12000 });
    if (!grid.beats.length) return;
    const beatPx = grid.beatDur * view.zoomPxPerSec;
    const barPx = grid.barDur * view.zoomPxPerSec;
    const subPx = beatPx / Math.max(1, tempo.subdivision);
    const barStride = barPx > 0 ? Math.max(1, Math.ceil((6 * scale) / barPx)) : 1;
    const drawBeats = beatPx >= 6 * scale;
    const drawSubs = tempo.subdivision > 1 && subPx >= 7 * scale;

    if (drawSubs) {
      ctx.save();
      ctx.strokeStyle = colors.gridSub;
      ctx.lineWidth = 1;
      ctx.setLineDash([1, 3]);
      ctx.beginPath();
      for (const time of grid.subdivisions) {
        const x = timeToX(time, view);
        if (x < -1 || x > width + 1) continue;
        ctx.moveTo(Math.round(x) + 0.5, 0);
        ctx.lineTo(Math.round(x) + 0.5, this.height);
      }
      ctx.stroke();
      ctx.restore();
    }

    if (drawBeats) {
      ctx.strokeStyle = colors.gridBeat;
      ctx.lineWidth = 1;
      ctx.beginPath();
      for (const beat of grid.beats) {
        if (beat.isBar) continue;
        const x = timeToX(beat.time, view);
        if (x < -1 || x > width + 1) continue;
        ctx.moveTo(Math.round(x) + 0.5, 0);
        ctx.lineTo(Math.round(x) + 0.5, this.height);
      }
      ctx.stroke();
    }

    ctx.strokeStyle = colors.gridBar;
    ctx.lineWidth = 1;
    ctx.beginPath();
    const barLines = [];
    for (const bar of grid.bars) {
      if (barStride > 1 && euclideanMod(bar.k, tempo.beatsPerBar * barStride) !== 0) continue;
      const x = timeToX(bar.time, view);
      if (x < -1 || x > width + 1) continue;
      barLines.push({ x, bar: bar.bar });
      ctx.moveTo(Math.round(x) + 0.5, 0);
      ctx.lineTo(Math.round(x) + 0.5, this.height);
    }
    ctx.stroke();

    // bar numbers at the top of the master lane
    const master = bounds[0];
    if (!master || master.height < 58 * scale || barPx * barStride < 40 * scale) return;
    const fontSize = Math.max(8, Math.round(9.5 * scale));
    ctx.save();
    ctx.font = `${fontSize}px ${MONO}`;
    ctx.textBaseline = 'middle';
    ctx.textAlign = 'left';
    ctx.fillStyle = colors.muted;
    let lastX = -Infinity;
    const minGap = 30 * scale;
    for (const item of barLines) {
      if (item.x - lastX < minGap) continue;
      if (item.x > width - 6 * scale) continue;
      ctx.fillText(String(item.bar), Math.round(item.x) + 4 * scale, master.top + 10 * scale);
      lastX = item.x;
    }
    ctx.restore();
  }

  ensureMasterCache(view, gain) {
    const width = this.width;
    const start = view.scrollSec;
    const end = start + width / view.zoomPxPerSec;
    const key = `m|${this.dataVersion}|${start.toFixed(6)}|${end.toFixed(6)}|${width}|${gain}`;
    if (this.cache.key === key && this.cache.master) return this.cache.master;
    const analysis = this.state.analysis;
    const min = new Float32Array(width);
    const max = new Float32Array(width);
    const peaksMin = analysis?.peaksMin;
    const peaksMax = analysis?.peaksMax;
    const rate = Number(analysis?.peakBucketsPerSecond) || 500;
    const count = Math.min(Array.isArray(peaksMin) ? peaksMin.length : 0, Array.isArray(peaksMax) ? peaksMax.length : 0);
    if (count > 0) {
      const duration = this.duration;
      for (let x = 0; x < width; x += 1) {
        const t0 = start + x / view.zoomPxPerSec;
        const t1 = start + (x + 1) / view.zoomPxPerSec;
        if (t1 <= 0 || t0 >= duration) continue;
        let i0 = Math.floor(Math.max(0, t0) * rate);
        let i1 = Math.ceil(Math.min(duration, t1) * rate) - 1;
        i0 = clamp(i0, 0, count - 1);
        i1 = clamp(i1, 0, count - 1);
        if (i1 < i0) i1 = i0;
        let lo = 0;
        let hi = 0;
        for (let i = i0; i <= i1; i += 1) {
          const a = Number(peaksMin[i]);
          const b = Number(peaksMax[i]);
          if (Number.isFinite(a) && a < lo) lo = a;
          if (Number.isFinite(b) && b > hi) hi = b;
        }
        min[x] = clamp(lo * gain, -1, 1);
        max[x] = clamp(hi * gain, -1, 1);
      }
    }
    this.cache = { ...this.cache, key, master: { min, max } };
    return this.cache.master;
  }

  drawMaster(ctx, colors, view, bound, scale) {
    if (!bound) return;
    const gain = clamp(Number(this.state.settings?.waveformGain) || 1, 0.25, 4);
    const data = this.ensureMasterCache(view, gain);
    const width = this.width;
    const mid = bound.top + bound.height / 2;
    const half = Math.max(1, bound.height / 2 - 6 * scale);

    ctx.beginPath();
    for (let x = 0; x < width; x += 1) {
      const y = mid - data.max[x] * half;
      if (x === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    }
    for (let x = width - 1; x >= 0; x -= 1) {
      const y = mid - data.min[x] * half;
      ctx.lineTo(x, y);
    }
    ctx.closePath();
    ctx.fillStyle = colors.wave;
    ctx.globalAlpha = 0.9;
    ctx.fill();
    ctx.globalAlpha = 1;
    this.centerLine(ctx, colors, bound.top, bound.height, 0.18);
  }

  ensureBandCache(band, index, view) {
    const width = this.width;
    const start = view.scrollSec;
    const end = start + width / view.zoomPxPerSec;
    const key = `b${index}|${this.dataVersion}|${start.toFixed(6)}|${end.toFixed(6)}|${width}|${band?.onset?.length || 0}`;
    const cached = this.cache.bands[index];
    if (cached && cached.key === key) return cached.data;
    const data = new Float32Array(width);
    const env = band?.envelope;
    const hop = Number(this.state.analysis?.frameHopSec) || 0;
    if (Array.isArray(env) && env.length && hop > 0) {
      const count = env.length;
      for (let x = 0; x < width; x += 1) {
        const t0 = start + x / view.zoomPxPerSec;
        const t1 = start + (x + 1) / view.zoomPxPerSec;
        if (t1 <= 0) continue;
        let i0 = Math.floor(Math.max(0, t0) / hop);
        let i1 = Math.ceil(t1 / hop) - 1;
        i0 = clamp(i0, 0, count - 1);
        i1 = clamp(i1, 0, count - 1);
        if (i1 < i0) i1 = i0;
        let peak = 0;
        for (let i = i0; i <= i1; i += 1) {
          const value = Number(env[i]);
          if (Number.isFinite(value) && value > peak) peak = value;
        }
        data[x] = clamp(peak, 0, 1);
      }
    }
    this.cache.bands[index] = { key, data };
    return data;
  }

  drawBand(ctx, colors, band, index, bound, view, scale) {
    if (!bound || !band) return;
    const data = this.ensureBandCache(band, index, view);
    const width = this.width;
    const mid = bound.top + bound.height / 2;
    const half = Math.max(1, bound.height / 2 - 5 * scale);
    const color = band.color || '#4FC3F7';

    ctx.beginPath();
    for (let x = 0; x < width; x += 1) {
      const y = mid - data[x] * half;
      if (x === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    }
    for (let x = width - 1; x >= 0; x -= 1) {
      ctx.lineTo(x, mid + data[x] * half);
    }
    ctx.closePath();
    ctx.fillStyle = hexToRgba(color, 0.22);
    ctx.fill();
    ctx.strokeStyle = hexToRgba(color, 0.8);
    ctx.lineWidth = 1;
    ctx.stroke();
    this.centerLine(ctx, colors, bound.top, bound.height, 0.14);
  }

  centerLine(ctx, colors, top, height, alpha) {
    const y = top + height / 2;
    ctx.save();
    ctx.strokeStyle = hexToRgba(colors.ink, alpha);
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(0, Math.round(y) + 0.5);
    ctx.lineTo(this.width, Math.round(y) + 0.5);
    ctx.stroke();
    ctx.restore();
  }

  drawPlayhead(ctx, colors, view, width, height, scale) {
    const position = Number(this.state.playback?.positionSec) || 0;
    const x = timeToX(position, view);
    if (x < -1 || x > width + 1) return;
    const px = Math.round(x) + 0.5;
    ctx.save();
    ctx.strokeStyle = 'rgba(0, 0, 0, 0.28)';
    ctx.lineWidth = 3;
    ctx.beginPath();
    ctx.moveTo(px, 0);
    ctx.lineTo(px, height);
    ctx.stroke();
    ctx.strokeStyle = colors.action;
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(px, 0);
    ctx.lineTo(px, height);
    ctx.stroke();
    ctx.restore();
  }

  drawHover(ctx, colors, view, width, height, scale) {
    if (this.hoverTime == null || !Number.isFinite(this.hoverTime)) return;
    const x = timeToX(this.hoverTime, view);
    if (x < 0 || x > width) return;
    ctx.save();
    ctx.strokeStyle = colors.hover;
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(Math.round(x) + 0.5, 0);
    ctx.lineTo(Math.round(x) + 0.5, height);
    ctx.stroke();
    ctx.restore();
  }

  /* ------------------------------------------------------------------ */
  /* ruler                                                              */
  /* ------------------------------------------------------------------ */

  drawRuler(colors) {
    const ctx = this.rulerCtx;
    const width = this.rulerW;
    const height = this.rulerH;
    const scale = this.scale;
    const view = this.view;
    const start = view.scrollSec;
    const end = start + width / view.zoomPxPerSec;

    ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    ctx.clearRect(0, 0, width, height);
    ctx.fillStyle = colors.surface;
    ctx.fillRect(0, 0, width, height);

    const step = chooseTickStep(view.zoomPxPerSec, 64 * scale);
    const ticks = generateTicks(start, end, step);
    const minorStep = step / 5;
    const drawMinor = minorStep * view.zoomPxPerSec >= 8 * scale;
    const fontSize = Math.max(8, Math.round(9.5 * scale));

    if (drawMinor) {
      ctx.strokeStyle = hexToRgba(colors.ink, 0.14);
      ctx.lineWidth = 1;
      ctx.beginPath();
      for (const time of generateTicks(start, end, minorStep)) {
        const x = timeToX(time, view);
        if (x < -1 || x > width + 1) continue;
        ctx.moveTo(Math.round(x) + 0.5, height - 4 * scale);
        ctx.lineTo(Math.round(x) + 0.5, height - 1);
      }
      ctx.stroke();
    }

    ctx.strokeStyle = hexToRgba(colors.ink, 0.4);
    ctx.lineWidth = 1;
    ctx.beginPath();
    for (const time of ticks) {
      const x = timeToX(time, view);
      if (x < -1 || x > width + 1) continue;
      ctx.moveTo(Math.round(x) + 0.5, height - 7 * scale);
      ctx.lineTo(Math.round(x) + 0.5, height - 1);
    }
    ctx.stroke();

    ctx.save();
    ctx.font = `${fontSize}px ${MONO}`;
    ctx.textBaseline = 'middle';
    ctx.textAlign = 'left';
    ctx.fillStyle = colors.muted;
    for (const time of ticks) {
      const x = timeToX(time, view);
      if (x < -1 || x > width - 4 * scale) continue;
      const decimals = step < 1 ? 1 : 0;
      const label = formatTime(time, { decimals });
      if (x + ctx.measureText(label).width > width - 3 * scale) continue;
      // Labels sit below the bar-start flag so the flag never covers them,
      // even at 150% UI scale where the label is wider than the 0.5 s gap.
      ctx.fillText(label, Math.round(x) + 4 * scale, height * 0.62);
    }
    ctx.restore();

    // flag handle
    const flagX = this.flagX();
    if (flagX != null && flagX >= -12 * scale && flagX <= width + 12 * scale) {
      const w = 1.25 * 16 * scale;
      const h = 12 * scale;
      const x = clamp(flagX - w / 2, 1, width - w - 1);
      const y = 1 * scale;
      ctx.save();
      ctx.fillStyle = colors.action;
      roundedRect(ctx, x, y, w, h, 3 * scale);
      ctx.fill();
      ctx.fillStyle = colors.onAction;
      ctx.font = `600 ${Math.max(8, Math.round(8.5 * scale))}px ${MONO}`;
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      ctx.fillText('1', x + w / 2, y + h / 2 + 0.5);
      ctx.restore();
    }

    // playhead line + handle
    const position = Number(this.state.playback?.positionSec) || 0;
    const px = timeToX(position, view);
    if (px >= -1 && px <= width + 1) {
      const x = Math.round(px) + 0.5;
      ctx.save();
      ctx.strokeStyle = colors.action;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(x, 0);
      ctx.lineTo(x, height);
      ctx.stroke();
      ctx.fillStyle = colors.action;
      ctx.beginPath();
      ctx.moveTo(x, height);
      ctx.lineTo(x - 4 * scale, height - 6 * scale);
      ctx.lineTo(x + 4 * scale, height - 6 * scale);
      ctx.closePath();
      ctx.fill();
      ctx.restore();
    }

    // hover time label
    if (this.hoverTime != null && Number.isFinite(this.hoverTime)) {
      const hx = timeToX(this.hoverTime, view);
      if (hx >= 0 && hx <= width) {
        const text = formatTime(Math.max(0, this.hoverTime), { decimals: 2 });
        ctx.save();
        ctx.font = `${Math.max(8, Math.round(9.5 * scale))}px ${MONO}`;
        const textWidth = ctx.measureText(text).width;
        const padX = 5 * scale;
        const labelW = textWidth + padX * 2;
        const labelH = 15 * scale;
        let left = hx + 7 * scale;
        if (left + labelW > width - 2) left = hx - labelW - 7 * scale;
        left = clamp(left, 2, Math.max(2, width - labelW - 2));
        const top = Math.max(2, (height - labelH) / 2 - 1);
        ctx.fillStyle = colors.surface2;
        roundedRect(ctx, left, top, labelW, labelH, 3 * scale);
        ctx.fill();
        ctx.strokeStyle = colors.rule;
        ctx.lineWidth = 1;
        ctx.stroke();
        ctx.fillStyle = colors.ink;
        ctx.textAlign = 'left';
        ctx.textBaseline = 'middle';
        ctx.fillText(text, left + padX, top + labelH / 2 + 0.5);
        ctx.restore();
      }
    }

    ctx.strokeStyle = colors.rule;
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(0, height - 0.5);
    ctx.lineTo(width, height - 0.5);
    ctx.stroke();
  }
}
