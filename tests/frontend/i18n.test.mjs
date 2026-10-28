import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import test from 'node:test';

import {
  applyI18n,
  dictionaries,
  flattenKeys,
  formatNumber,
  getLang,
  hasKey,
  interpolate,
  setLang,
  t,
} from '../../ui/js/i18n.js';

const here = path.dirname(fileURLToPath(import.meta.url));
const html = readFileSync(path.join(here, '..', '..', 'ui', 'index.html'), 'utf8');

function placeholders(text) {
  return [...String(text).matchAll(/\{([A-Za-z0-9_]+)\}/g)]
    .map((match) => match[1])
    .sort();
}

test('ja and en dictionaries expose exactly the same keys', () => {
  const ja = flattenKeys('ja');
  const en = flattenKeys('en');
  assert.ok(ja.length > 0);
  assert.deepEqual(ja, en);
});

test('every dictionary value is a non-empty string', () => {
  for (const lang of ['ja', 'en']) {
    for (const key of flattenKeys(lang)) {
      const value = dictionaries[lang][key];
      assert.equal(typeof value, 'string', `${lang}:${key} must be a string`);
      assert.ok(value.trim().length > 0, `${lang}:${key} must not be empty`);
    }
  }
});

test('ja and en keep the same interpolation placeholders', () => {
  for (const key of flattenKeys('ja')) {
    assert.deepEqual(
      placeholders(dictionaries.ja[key]),
      placeholders(dictionaries.en[key]),
      `placeholder mismatch for ${key}`,
    );
  }
});

test('interpolate replaces known placeholders and leaves unknown ones', () => {
  assert.equal(interpolate('{a} + {b} = {c}', { a: 1, b: 2, c: 3 }), '1 + 2 = 3');
  assert.equal(interpolate('{a}{a}', { a: 'x' }), 'xx');
  assert.equal(interpolate('{missing}', {}), '{missing}');
  assert.equal(interpolate('{value}', { value: null }), '');
  assert.equal(interpolate('plain', { a: 1 }), 'plain');
  assert.equal(interpolate(null, { a: 1 }), null);
});

test('t switches language, falls back to en and warns only once', () => {
  const originalWarn = console.warn;
  const warnings = [];
  console.warn = (message) => warnings.push(String(message));
  try {
    setLang('ja');
    assert.equal(getLang(), 'ja');
    assert.equal(t('transport.play'), dictionaries.ja['transport.play']);
    setLang('en');
    assert.equal(t('transport.play'), dictionaries.en['transport.play']);
    setLang('unknown');
    assert.equal(getLang(), 'ja');
    assert.equal(t('__missing.key__'), '__missing.key__');
    assert.equal(t('__missing.key__'), '__missing.key__');
    assert.equal(warnings.filter((line) => line.includes('__missing.key__')).length, 1);
  } finally {
    console.warn = originalWarn;
    setLang('ja');
  }
});

test('hasKey only reports dictionary keys', () => {
  assert.equal(hasKey('transport.play'), true);
  assert.equal(hasKey('error.ERR_UNKNOWN'), true);
  assert.equal(hasKey('__nope__'), false);
  assert.equal(hasKey(''), false);
});

test('formatNumber follows the active language', () => {
  setLang('en');
  assert.equal(formatNumber(1234.5), '1,234.5');
  setLang('ja');
  assert.equal(formatNumber(1234.5), '1,234.5');
  assert.equal(formatNumber(Number.NaN), '—');
});

test('every data-i18n key in index.html exists in both dictionaries', () => {
  const keys = new Set();
  for (const match of html.matchAll(/data-i18n(?:-[a-z-]+)?="([^"]+)"/g)) {
    keys.add(match[1]);
  }
  assert.ok(keys.size > 50, 'expected the UI to be fully localised');
  for (const key of keys) {
    assert.ok(hasKey(key), `missing i18n key: ${key}`);
    assert.ok(dictionaries.ja[key], `missing ja value: ${key}`);
    assert.ok(dictionaries.en[key], `missing en value: ${key}`);
  }
});

test('applyI18n is a no-op without a DOM', () => {
  assert.doesNotThrow(() => applyI18n(null));
});
