/**
 * Dialog controllers. Native <dialog> supplies the modal focus trap; this
 * module adds backdrop/Esc close handling, focus restoration and the live
 * tempo/settings form logic.
 */

import { t } from './i18n.js';
import {
  clamp,
  crossoversFromBands,
  formatTime,
  validateCrossovers,
} from './core.js';

export class DialogController {
  constructor() {
    this.returns = new WeakMap();
    this.bound = new WeakSet();
  }

  bind(dialog) {
    if (!dialog || this.bound.has(dialog)) return;
    this.bound.add(dialog);
    dialog.addEventListener('close', () => {
      const previous = this.returns.get(dialog);
      this.returns.delete(dialog);
      const callback = dialog._nvOnClose;
      dialog._nvOnClose = null;
      if (typeof callback === 'function') callback();
      if (previous && previous.focus && document.contains(previous)) {
        previous.focus({ preventScroll: true });
      }
    });
    dialog.addEventListener('pointerdown', (event) => {
      if (event.target === dialog) dialog.close();
    });
  }

  open(dialog, { onClose = null, initialFocus = null } = {}) {
    if (!dialog) return;
    this.bind(dialog);
    const previous = document.activeElement;
    this.returns.set(dialog, previous);
    dialog._nvOnClose = onClose;
    if (!dialog.open) dialog.showModal();
    requestAnimationFrame(() => {
      if (!dialog.open) return;
      let target = null;
      if (typeof initialFocus === 'string') target = dialog.querySelector(initialFocus);
      else if (initialFocus && typeof initialFocus.focus === 'function') target = initialFocus;
      if (!target) target = dialog.querySelector('[autofocus]');
      if (!target) target = dialog.querySelector('button, input, select, textarea');
      if (target) target.focus({ preventScroll: true });
    });
  }

  close(dialog) {
    if (dialog && dialog.open) dialog.close();
  }

  isOpen(dialog) {
    return Boolean(dialog && dialog.open);
  }
}

/* ------------------------------------------------------------------ */
/* tempo                                                               */
/* ------------------------------------------------------------------ */

export class TempoDialog {
  constructor(dialog, controller, options = {}) {
    this.el = dialog;
    this.controller = controller;
    this.onApply = typeof options.onApply === 'function' ? options.onApply : () => {};
    this.getDuration = typeof options.getDuration === 'function' ? options.getDuration : () => 0;
    this.getPlayhead = typeof options.getPlayhead === 'function' ? options.getPlayhead : () => 0;
    this.duration = 0;
    this.playheadSec = 0;
    this.taps = [];
    this.bind();
  }

  bind() {
    const q = (id) => this.el.querySelector(id);
    this.bpm = q('#tempo-bpm');
    this.barStart = q('#tempo-bar-start');
    this.showGrid = q('#tempo-show-grid');
    this.snap = q('#tempo-snap');
    this.error = q('#tempo-error');

    q('#bpm-dec').addEventListener('click', () => this.stepBpm(-1));
    q('#bpm-inc').addEventListener('click', () => this.stepBpm(1));
    q('#tap-btn').addEventListener('click', () => this.tap());
    q('#use-playhead-btn').addEventListener('click', () => {
      this.barStart.value = this.getPlayhead().toFixed(3);
      this.clearError();
    });
    q('#tempo-cancel').addEventListener('click', () => this.controller.close(this.el));
    q('#tempo-close').addEventListener('click', () => this.controller.close(this.el));
    q('#tempo-apply').addEventListener('click', () => this.apply());
    this.el.addEventListener('keydown', (event) => {
      if (event.key === 'Enter' && event.target && event.target.tagName === 'INPUT') {
        event.preventDefault();
        this.apply();
      }
    });
    for (const input of [this.bpm, this.barStart]) {
      input.addEventListener('input', () => this.clearError());
    }
    for (const radio of this.el.querySelectorAll('input[name="tempo-meter"]')) {
      radio.addEventListener('change', () => this.clearError());
    }
  }

  stepBpm(delta) {
    const current = Number(this.bpm.value);
    const value = clamp((Number.isFinite(current) ? current : 120) + delta, 20, 400);
    this.bpm.value = String(Math.round(value * 10) / 10);
    this.clearError();
  }

  tap() {
    const now = performance.now();
    if (this.taps.length && now - this.taps[this.taps.length - 1] > 2500) this.taps = [];
    this.taps.push(now);
    if (this.taps.length > 5) this.taps.shift();
    if (this.taps.length < 2) return;
    const intervals = [];
    for (let i = 1; i < this.taps.length; i += 1) intervals.push(this.taps[i] - this.taps[i - 1]);
    const recent = intervals.slice(-4);
    const avg = recent.reduce((a, b) => a + b, 0) / recent.length;
    if (!(avg > 0)) return;
    const bpm = clamp(60000 / avg, 20, 400);
    this.bpm.value = String(Math.round(bpm * 10) / 10);
    this.clearError();
  }

  setRadio(name, value) {
    for (const radio of this.el.querySelectorAll(`input[name="${name}"]`)) {
      radio.checked = Number(radio.value) === Number(value);
    }
  }

  open(tempo, opts = {}) {
    this.duration = Math.max(0, Number(opts.duration ?? this.getDuration()) || 0);
    this.playheadSec = Math.max(0, Number(opts.playheadSec ?? this.getPlayhead()) || 0);
    const source = tempo || {};
    this.bpm.value = String(clamp(source.bpm ?? 120, 20, 400));
    this.setRadio('tempo-meter', source.beatsPerBar ?? 4);
    this.setRadio('tempo-sub', source.subdivision ?? 1);
    const barStart = opts.barStartOverride != null ? opts.barStartOverride : (source.barStartSec ?? 0);
    this.barStart.value = String(clamp(barStart, 0, Math.max(0, this.duration)).toFixed(3));
    this.showGrid.checked = source.showGrid !== false;
    this.snap.checked = source.snapToGrid === true;
    this.taps = [];
    this.clearError();
    this.controller.open(this.el, { initialFocus: this.bpm });
    requestAnimationFrame(() => {
      if (!this.el.open) return;
      this.bpm.focus({ preventScroll: true });
      this.bpm.select();
    });
  }

  showError(message) {
    this.error.textContent = message;
    this.error.hidden = false;
  }

  clearError() {
    this.error.textContent = '';
    this.error.hidden = true;
  }

  apply() {
    const bpm = Number(this.bpm.value);
    if (!Number.isFinite(bpm) || bpm < 20 || bpm > 400) {
      this.showError(t('tempo.errorBpm'));
      this.bpm.focus();
      return;
    }
    const barStart = Number(this.barStart.value);
    if (!Number.isFinite(barStart) || barStart < 0) {
      this.showError(t('tempo.errorBarStart', { duration: formatTime(this.duration, { decimals: 3 }) }));
      this.barStart.focus();
      return;
    }
    if (this.duration > 0 && barStart > this.duration + 1e-6) {
      this.showError(t('tempo.errorBarStart', { duration: formatTime(this.duration, { decimals: 3 }) }));
      this.barStart.focus();
      return;
    }
    const meter = Number(this.el.querySelector('input[name="tempo-meter"]:checked')?.value || 4);
    const subdivision = Number(this.el.querySelector('input[name="tempo-sub"]:checked')?.value || 1);
    const tempo = {
      enabled: true,
      bpm: Math.round(bpm * 10) / 10,
      beatsPerBar: [2, 3, 4, 6, 8].includes(meter) ? meter : 4,
      barStartSec: Math.round(clamp(barStart, 0, this.duration > 0 ? this.duration : barStart) * 1000) / 1000,
      subdivision: [1, 2, 4].includes(subdivision) ? subdivision : 1,
      showGrid: this.showGrid.checked,
      snapToGrid: this.snap.checked,
    };
    this.controller.close(this.el);
    this.onApply(tempo);
  }
}

/* ------------------------------------------------------------------ */
/* settings                                                            */
/* ------------------------------------------------------------------ */

export class SettingsDialog {
  constructor(dialog, controller, options = {}) {
    this.el = dialog;
    this.controller = controller;
    this.onChange = typeof options.onChange === 'function' ? options.onChange : () => {};
    this.onBandsChange = typeof options.onBandsChange === 'function' ? options.onBandsChange : () => {};
    this.onFftRequest = typeof options.onFftRequest === 'function' ? options.onFftRequest : () => {};
    this.onClose = typeof options.onClose === 'function' ? options.onClose : null;
    this.currentFft = 2048;
    this.currentBandCount = 4;
    this.crossovers = [250, 1000, 4000];
    this.settings = null;
    this.appInfo = null;
    this.bind();
  }

  q(id) {
    return this.el.querySelector(id);
  }

  bind() {
    this.lang = this.q('#set-lang');
    this.scale = this.q('#set-scale');
    this.scaleOut = this.q('#set-scale-out');
    this.volume = this.q('#set-volume');
    this.volumeOut = this.q('#set-volume-out');
    this.gain = this.q('#set-gain');
    this.gainOut = this.q('#set-gain-out');
    this.follow = this.q('#set-follow');
    this.sensitivity = this.q('#set-sensitivity');
    this.sensitivityOut = this.q('#set-sensitivity-out');
    this.decimals = this.q('#set-decimals');
    this.undetected = this.q('#set-undetected');
    this.crossError = this.q('#set-cross-error');
    this.crossInputs = [this.q('#set-cross-1'), this.q('#set-cross-2'), this.q('#set-cross-3')];
    this.nav = this.q('.settings-nav');
    this.tabs = [...this.el.querySelectorAll('.settings-tab')];
    this.panes = [...this.el.querySelectorAll('.settings-pane')];

    this.q('#settings-close').addEventListener('click', () => this.controller.close(this.el));
    this.q('#settings-close-2').addEventListener('click', () => this.controller.close(this.el));

    for (const tab of this.tabs) {
      tab.addEventListener('click', () => this.activateTab(tab.dataset.pane));
    }

    this.lang.addEventListener('change', () => this.onChange({ lang: this.lang.value }));

    for (const radio of this.el.querySelectorAll('input[name="set-theme"]')) {
      radio.addEventListener('change', () => {
        if (radio.checked) this.onChange({ theme: radio.value });
      });
    }

    this.scale.addEventListener('input', () => {
      const percent = Number(this.scale.value);
      this.scaleOut.textContent = t('settings.scaleOut', { n: percent });
      this.paintRange(this.scale);
      this.onChange({ scale: percent / 100 });
    });

    this.volume.addEventListener('input', () => {
      const value = Number(this.volume.value);
      this.volumeOut.textContent = t('settings.scaleOut', { n: Math.round(value * 100) });
      this.paintRange(this.volume);
      this.onChange({ volume: value });
    });

    this.gain.addEventListener('input', () => {
      const value = Number(this.gain.value);
      this.gainOut.textContent = t('settings.gainOut', { n: this.formatGain(value) });
      this.paintRange(this.gain);
      this.onChange({ waveformGain: value });
    });

    this.follow.addEventListener('change', () => this.onChange({ followPlayhead: this.follow.checked }));

    for (const radio of this.el.querySelectorAll('input[name="set-fft"]')) {
      radio.addEventListener('change', () => {
        if (!radio.checked) return;
        const size = Number(radio.value);
        if (size === this.currentFft) return;
        this.onFftRequest(size, () => this.setRadio('set-fft', this.currentFft));
      });
    }

    for (const radio of this.el.querySelectorAll('input[name="set-band-count"]')) {
      radio.addEventListener('change', () => {
        if (!radio.checked) return;
        this.currentBandCount = Number(radio.value);
        this.updateCrossoverDisabled();
        this.applyCrossovers();
      });
    }

    for (const input of this.crossInputs) {
      input.addEventListener('change', () => this.applyCrossovers());
      input.addEventListener('input', () => this.clearCrossError());
    }

    this.sensitivity.addEventListener('input', () => {
      const value = Number(this.sensitivity.value);
      this.sensitivityOut.textContent = value.toFixed(2);
      this.paintRange(this.sensitivity);
      this.onChange({ onsetSensitivity: value });
    });

    for (const radio of this.el.querySelectorAll('input[name="set-delimiter"]')) {
      radio.addEventListener('change', () => {
        if (radio.checked) this.onChange({ csvDelimiter: radio.value });
      });
    }

    this.decimals.addEventListener('change', () => {
      this.onChange({ csvDecimals: Number(this.decimals.value) });
    });

    this.undetected.addEventListener('change', () => {
      this.onChange({ includeUndetected: this.undetected.checked });
    });

    this.nav.addEventListener('keydown', (event) => this.handleNavKey(event));
  }

  formatGain(value) {
    const rounded = Math.round(value * 100) / 100;
    return Number.isInteger(rounded) ? String(rounded) : String(rounded);
  }

  paintRange(input) {
    if (!input) return;
    const min = Number(input.min);
    const max = Number(input.max);
    const value = Number(input.value);
    const span = Number.isFinite(max) && Number.isFinite(min) ? max - min : 0;
    const percent = span > 0 && Number.isFinite(value)
      ? Math.min(100, Math.max(0, ((value - min) / span) * 100))
      : 0;
    input.style.setProperty('--fill', `${percent}%`);
  }

  setRadio(name, value) {
    for (const radio of this.el.querySelectorAll(`input[name="${name}"]`)) {
      radio.checked = String(radio.value) === String(value);
    }
  }

  activateTab(name, focus = false) {
    const tab = this.tabs.find((item) => item.dataset.pane === name) || this.tabs[0];
    if (!tab) return;
    const paneName = tab.dataset.pane;
    for (const item of this.tabs) {
      const active = item === tab;
      item.classList.toggle('is-active', active);
      item.setAttribute('aria-selected', active ? 'true' : 'false');
      item.tabIndex = active ? 0 : -1;
    }
    for (const pane of this.panes) {
      pane.hidden = pane.id !== `pane-${paneName}`;
      pane.classList.toggle('is-active', !pane.hidden);
    }
    if (focus) tab.focus({ preventScroll: true });
  }

  handleNavKey(event) {
    const keys = ['ArrowDown', 'ArrowUp', 'Home', 'End'];
    if (!keys.includes(event.key)) return;
    event.preventDefault();
    const index = this.tabs.indexOf(document.activeElement);
    let next = index;
    if (event.key === 'ArrowDown') next = (index + 1 + this.tabs.length) % this.tabs.length;
    else if (event.key === 'ArrowUp') next = (index - 1 + this.tabs.length) % this.tabs.length;
    else if (event.key === 'Home') next = 0;
    else if (event.key === 'End') next = this.tabs.length - 1;
    const tab = this.tabs[next];
    if (tab) this.activateTab(tab.dataset.pane, true);
  }

  open(settings, appInfo = null, opts = {}) {
    this.appInfo = appInfo;
    this.sync(settings);
    this.activateTab(opts.tab || 'general', false);
    this.controller.open(this.el, {
      initialFocus: this.el.querySelector('.settings-tab.is-active'),
      onClose: this.onClose,
    });
  }

  sync(settings) {
    if (!settings) return;
    this.settings = settings;
    this.currentFft = settings.fftSize;
    this.currentBandCount = settings.bandCount;
    this.crossovers = crossoversFromBands(settings.bands);
    this.lang.value = settings.lang;
    this.setRadio('set-theme', settings.theme);
    this.setRadio('set-fft', settings.fftSize);
    this.setRadio('set-band-count', settings.bandCount);
    this.setRadio('set-delimiter', settings.csvDelimiter);
    const percent = Math.round(settings.scale * 100);
    this.scale.value = String(percent);
    this.scaleOut.textContent = t('settings.scaleOut', { n: percent });
    this.volume.value = String(settings.volume);
    this.volumeOut.textContent = t('settings.scaleOut', { n: Math.round(settings.volume * 100) });
    this.gain.value = String(settings.waveformGain);
    this.gainOut.textContent = t('settings.gainOut', { n: this.formatGain(settings.waveformGain) });
    this.follow.checked = settings.followPlayhead;
    this.sensitivity.value = String(settings.onsetSensitivity);
    this.sensitivityOut.textContent = Number(settings.onsetSensitivity).toFixed(2);
    this.paintRange(this.scale);
    this.paintRange(this.volume);
    this.paintRange(this.gain);
    this.paintRange(this.sensitivity);
    this.decimals.value = String(settings.csvDecimals);
    this.undetected.checked = settings.includeUndetected;
    for (let i = 0; i < this.crossInputs.length; i += 1) {
      this.crossInputs[i].value = String(this.crossovers[i] ?? '');
    }
    this.updateCrossoverDisabled();
    this.clearCrossError();
    this.syncAbout();
  }

  syncAbout() {
    const version = this.appInfo?.version || '0.1.0';
    this.q('#about-version').textContent = version;
    this.q('#about-datadir').textContent = this.appInfo?.dataDir || '—';
    const portable = this.appInfo?.portable;
    this.q('#about-portable').textContent = portable == null
      ? '—'
      : t(portable ? 'settings.about.portableYes' : 'settings.about.portableNo');
  }

  updateCrossoverDisabled() {
    for (let i = 0; i < this.crossInputs.length; i += 1) {
      this.crossInputs[i].disabled = i >= this.currentBandCount - 1;
    }
  }

  clearCrossError() {
    this.crossError.textContent = '';
    this.crossError.hidden = true;
  }

  showCrossError() {
    this.crossError.textContent = t('settings.crossoverError');
    this.crossError.hidden = false;
  }

  applyCrossovers() {
    const values = this.crossInputs.map((input) => Number(input.value));
    const result = validateCrossovers(values, this.currentBandCount);
    if (!result.ok) {
      this.showCrossError();
      return;
    }
    this.clearCrossError();
    this.crossovers = values;
    this.onBandsChange({
      bandCount: this.currentBandCount,
      crossovers: values.slice(0, Math.max(0, this.currentBandCount - 1)),
    });
  }

  setFft(size) {
    this.currentFft = Number(size);
    this.setRadio('set-fft', this.currentFft);
  }

  setBandCount(count) {
    this.currentBandCount = Number(count);
    this.setRadio('set-band-count', this.currentBandCount);
    this.updateCrossoverDisabled();
  }
}

/* ------------------------------------------------------------------ */
/* confirm                                                             */
/* ------------------------------------------------------------------ */

export class ConfirmDialog {
  constructor(dialog, controller) {
    this.el = dialog;
    this.controller = controller;
    this.pending = null;
    this.settled = false;
    this.titleEl = dialog.querySelector('#confirm-title');
    this.messageEl = dialog.querySelector('#confirm-message');
    this.ok = dialog.querySelector('#confirm-ok');
    this.cancel = dialog.querySelector('#confirm-cancel');
    this.ok.addEventListener('click', () => this.finish(true));
    this.cancel.addEventListener('click', () => this.finish(false));
  }

  ask({ title, message, confirmLabel, cancelLabel } = {}) {
    if (this.pending) {
      const previous = this.pending;
      this.pending = null;
      this.settled = true;
      this.controller.close(this.el);
      previous(false);
    }
    this.titleEl.textContent = title || t('common.confirm');
    this.messageEl.textContent = message || '';
    this.ok.textContent = confirmLabel || t('common.confirm');
    this.cancel.textContent = cancelLabel || t('common.cancel');
    this.settled = false;
    return new Promise((resolve) => {
      this.pending = resolve;
      this.controller.open(this.el, {
        initialFocus: this.cancel,
        onClose: () => {
          if (this.settled) return;
          this.settled = true;
          this.pending = null;
          resolve(false);
        },
      });
    });
  }

  finish(value) {
    if (!this.pending) return;
    this.settled = true;
    const resolve = this.pending;
    this.pending = null;
    this.controller.close(this.el);
    resolve(Boolean(value));
  }
}

