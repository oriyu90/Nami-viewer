/**
 * Custom HTML context menu (no native menus).
 * role=menu / role=menuitem, keyboard navigation, viewport flipping and
 * focus restoration to the invoking element.
 */

import { t } from './i18n.js';

const FOCUSABLE = 'button.ctx-item:not([aria-disabled="true"])';

export class ContextMenu {
  constructor(root, options = {}) {
    this.root = root;
    this.onClose = typeof options.onClose === 'function' ? options.onClose : null;
    this.el = null;
    this.invoker = null;
    this.items = [];
    this.focusedIndex = -1;
    this.bound = null;
    this.closeTimer = null;
  }

  get isOpen() {
    return Boolean(this.el);
  }

  open({ x = 0, y = 0, items = [], header = null, footer = null, invoker = null, anchor = null, align = 'start' } = {}) {
    this.close({ restoreFocus: false, silent: true });
    this.items = items;
    this.invoker = invoker;
    this.focusedIndex = -1;

    const el = document.createElement('div');
    el.className = 'ctx-menu';
    el.setAttribute('role', 'menu');
    el.setAttribute('aria-label', t('menu.label'));
    el.tabIndex = -1;

    if (header && (header.label || header.meta)) {
      const head = document.createElement('div');
      head.className = 'ctx-header';
      head.setAttribute('role', 'presentation');
      if (header.color) {
        const dot = document.createElement('span');
        dot.className = 'ctx-header-dot';
        dot.style.setProperty('--lane-color', header.color);
        dot.setAttribute('aria-hidden', 'true');
        head.appendChild(dot);
      }
      const copy = document.createElement('div');
      copy.className = 'ctx-header-copy';
      if (header.label) {
        const label = document.createElement('span');
        label.className = 'ctx-header-label';
        label.textContent = header.label;
        copy.appendChild(label);
      }
      if (header.meta) {
        const meta = document.createElement('span');
        meta.className = 'ctx-header-meta';
        meta.textContent = header.meta;
        copy.appendChild(meta);
      }
      head.appendChild(copy);
      el.appendChild(head);
    }

    items.forEach((item, index) => {
      if (item.separator) {
        const sep = document.createElement('div');
        sep.className = 'ctx-sep';
        sep.setAttribute('role', 'separator');
        el.appendChild(sep);
        return;
      }
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'ctx-item';
      button.setAttribute('role', 'menuitem');
      button.tabIndex = -1;
      button.dataset.index = String(index);
      if (item.disabled) button.setAttribute('aria-disabled', 'true');
      if (item.icon) {
        const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
        svg.setAttribute('class', 'icon');
        svg.setAttribute('aria-hidden', 'true');
        const use = document.createElementNS('http://www.w3.org/2000/svg', 'use');
        use.setAttribute('href', `#${item.icon}`);
        svg.appendChild(use);
        button.appendChild(svg);
      }
      const label = document.createElement('span');
      label.textContent = item.label;
      button.appendChild(label);
      button.addEventListener('click', () => this.activate(index));
      el.appendChild(button);
    });

    if (footer) {
      const foot = document.createElement('div');
      foot.className = 'ctx-footer';
      foot.setAttribute('role', 'note');
      const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
      svg.setAttribute('class', 'icon');
      svg.setAttribute('aria-hidden', 'true');
      const use = document.createElementNS('http://www.w3.org/2000/svg', 'use');
      use.setAttribute('href', '#icon-warning');
      svg.appendChild(use);
      foot.appendChild(svg);
      const text = document.createElement('span');
      text.textContent = footer;
      foot.appendChild(text);
      el.appendChild(foot);
    }

    this.root.appendChild(el);
    this.el = el;
    this.position(el, { x, y, anchor, align });

    if (invoker && invoker.setAttribute && invoker.hasAttribute('aria-haspopup')) {
      invoker.setAttribute('aria-expanded', 'true');
    }

    this.bind(el);
    const first = el.querySelector(FOCUSABLE);
    if (first) {
      first.focus({ preventScroll: true });
      this.focusedIndex = Number(first.dataset.index);
    } else {
      el.focus({ preventScroll: true });
    }
    return el;
  }

  position(el, { x, y, anchor, align }) {
    const margin = 8;
    const rect = el.getBoundingClientRect();
    let left = x;
    let top = y;
    if (anchor) {
      left = align === 'end' ? anchor.right - rect.width : anchor.left;
      top = anchor.bottom + 4;
      if (top + rect.height > window.innerHeight - margin) {
        top = Math.max(margin, anchor.top - rect.height - 4);
      }
    } else {
      if (left + rect.width > window.innerWidth - margin) left = Math.max(margin, left - rect.width);
      if (top + rect.height > window.innerHeight - margin) top = Math.max(margin, top - rect.height);
    }
    left = Math.min(Math.max(margin, left), Math.max(margin, window.innerWidth - rect.width - margin));
    top = Math.min(Math.max(margin, top), Math.max(margin, window.innerHeight - rect.height - margin));
    el.style.left = `${Math.round(left)}px`;
    el.style.top = `${Math.round(top)}px`;
  }

  bind(el) {
    const onKey = (event) => this.handleKey(event);
    const onDocPointer = (event) => {
      if (!this.el) return;
      if (!this.el.contains(event.target)) this.close({ restoreFocus: false });
    };
    const onViewport = () => this.close({ restoreFocus: false });
    el.addEventListener('keydown', onKey);
    document.addEventListener('pointerdown', onDocPointer, true);
    window.addEventListener('resize', onViewport);
    window.addEventListener('scroll', onViewport, true);
    this.bound = { el, onKey, onDocPointer, onViewport };
  }

  handleKey(event) {
    if (!this.el) return;
    const focusables = [...this.el.querySelectorAll(FOCUSABLE)];
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      this.close({ restoreFocus: true });
      return;
    }
    if (event.key === 'Tab') {
      event.preventDefault();
      this.close({ restoreFocus: true });
      return;
    }
    if (focusables.length === 0) return;
    const current = focusables.indexOf(document.activeElement);
    let next = -1;
    if (event.key === 'ArrowDown') next = current < 0 ? 0 : (current + 1) % focusables.length;
    else if (event.key === 'ArrowUp') next = current <= 0 ? focusables.length - 1 : current - 1;
    else if (event.key === 'Home') next = 0;
    else if (event.key === 'End') next = focusables.length - 1;
    else if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      if (current >= 0) this.activate(Number(focusables[current].dataset.index));
      return;
    } else {
      return;
    }
    event.preventDefault();
    const target = focusables[next];
    target.focus({ preventScroll: true });
    this.focusedIndex = Number(target.dataset.index);
  }

  activate(index) {
    const item = this.items[index];
    if (!item || item.separator || item.disabled) return;
    this.close({ restoreFocus: true });
    if (typeof item.action === 'function') {
      try {
        item.action();
      } catch (error) {
        console.error('[menu] action failed', error);
      }
    }
  }

  close({ restoreFocus = false, silent = false } = {}) {
    if (!this.el) return;
    const el = this.el;
    const invoker = this.invoker;
    if (this.bound) {
      el.removeEventListener('keydown', this.bound.onKey);
      document.removeEventListener('pointerdown', this.bound.onDocPointer, true);
      window.removeEventListener('resize', this.bound.onViewport);
      window.removeEventListener('scroll', this.bound.onViewport, true);
    }
    el.remove();
    this.el = null;
    this.bound = null;
    this.items = [];
    this.focusedIndex = -1;
    this.invoker = null;
    if (invoker && invoker.setAttribute && invoker.hasAttribute('aria-haspopup')) {
      invoker.setAttribute('aria-expanded', 'false');
    }
    if (restoreFocus && invoker && typeof invoker.focus === 'function' && document.contains(invoker)) {
      invoker.focus({ preventScroll: true });
    }
    if (!silent && this.onClose) this.onClose();
  }
}
