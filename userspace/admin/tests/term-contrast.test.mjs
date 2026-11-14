// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The terminal's colours (design 13.11): read from term.css, so the file is
// what is tested, and checked for contrast by the WCAG 2.2 formula.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import {
  MINIMUM, contrast as shownContrast, parseColour, readable,
} from '../pages/terminal/contrast-model.js';

const whole = readFileSync(new URL('../pages/terminal/term.css', import.meta.url), 'utf8');
// The file starts with the tokens; the page's own rules follow a comment that begins "The page".
const css = whole.split('\n/* The page')[0];

// The rules of the file as a map from selector to its declarations of
// custom properties. Comments go first; a media query's rules are listed
// under `@media (...) selector`.
function rules(source) {
  const result = new Map();
  const text = source.replace(/\/\*[\s\S]*?\*\//g, '');
  const add = (prefix, block) => {
    for (const [, selector, body] of block.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
      const key = `${prefix}${selector.trim().replace(/\s+/g, ' ')}`;
      const declarations = result.get(key) ?? {};
      for (const [, name, value] of body.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) declarations[name] = value.trim();
      result.set(key, declarations);
    }
  };
  const media = /@media\s*([^{]+)\{((?:[^{}]*\{[^{}]*\})*)\s*\}/g;
  add('', text.replace(media, ''));
  for (const [, query, inner] of text.matchAll(media)) add(`@media ${query.trim()} `, inner);
  return result;
}

const sheet = rules(css);
const THEMES = { light: '.dk', dark: '.dk.dark', hc: '.dk.hc' };

// The theme's tokens: the base's, with the theme's own over them.
const tokens = (theme) => ({ ...sheet.get('.dk'), ...sheet.get(THEMES[theme]) });

const channel = (value) => {
  const c = value / 255;
  return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
};
function luminance(hex) {
  assert.match(hex, /^#[0-9A-Fa-f]{6}$/, `${hex} is a #RRGGBB colour`);
  const n = parseInt(hex.slice(1), 16);
  return 0.2126 * channel(n >> 16) + 0.7152 * channel((n >> 8) & 255) + 0.0722 * channel(n & 255);
}
function contrast(a, b) {
  const [high, low] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (high + 0.05) / (low + 0.05);
}

test('the contrast formula gives the known values', () => {
  assert.equal(contrast('#000000', '#FFFFFF'), 21);
  assert.equal(contrast('#FFFFFF', '#FFFFFF'), 1);
  assert.ok(Math.abs(contrast('#767676', '#FFFFFF') - 4.54) < 0.01);
  assert.ok(Math.abs(contrast('#777777', '#FFFFFF') - 4.48) < 0.01);
});

test('each theme has every token, as a colour', () => {
  for (const theme of Object.keys(THEMES)) {
    const set = tokens(theme);
    const names = [...Array.from({ length: 16 }, (_, i) => `--term-${i}`), '--term-fg', '--term-bg', '--term-cursor', '--term-cursor-text', '--term-sel-bg', '--term-sel-fg'];
    for (const name of names) assert.match(set[name] ?? '', /^#[0-9A-F]{6}$/, `${theme} ${name}`);
  }
});

test('the page rules use only tokens the themes give', () => {
  const rest = whole.slice(css.length);
  assert.ok(rest.length > 0);
  const given = new Set(Object.keys(tokens('light')));
  for (const [, name] of rest.matchAll(/var\((--term-[\w-]+)\)/g)) assert.ok(given.has(name), name);
  for (const theme of Object.keys(THEMES)) assert.deepEqual(Object.keys(tokens(theme)).sort(), [...given].sort(), theme);
});

test('the token part of the file holds tokens only', () => {
  const text = css.replace(/\/\*[\s\S]*?\*\//g, '');
  // The names before a colon inside the braces of a rule.
  const declarations = [...text.matchAll(/\{([^{}]*)\}/g)].flatMap(([, body]) => [...body.matchAll(/([\w-]+)\s*:/g)].map((match) => match[1]));
  assert.ok(declarations.length > 0);
  for (const name of declarations) assert.match(name, /^--term-/, name);
  assert.deepEqual([...sheet.keys()], ['.dk', '.dk.dark', '.dk.hc', '@media (forced-colors: active) .dk, .dk.dark, .dk.hc']);
});

test('colours 1 to 7 and 9 to 15 are at least 4.5:1 against the background in each theme', () => {
  for (const theme of Object.keys(THEMES)) {
    const set = tokens(theme);
    for (const n of [1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 12, 13, 14, 15]) {
      const ratio = contrast(set[`--term-${n}`], set['--term-bg']);
      assert.ok(ratio >= 4.5, `${theme} colour ${n} is ${ratio.toFixed(2)}:1`);
    }
  }
});

test('the foreground is at least 7:1 against the background in each theme', () => {
  for (const theme of Object.keys(THEMES)) {
    const set = tokens(theme);
    const ratio = contrast(set['--term-fg'], set['--term-bg']);
    assert.ok(ratio >= 7, `${theme} foreground is ${ratio.toFixed(2)}:1`);
  }
});

test('bright black is readable too, and the colours are told apart', () => {
  for (const theme of Object.keys(THEMES)) {
    const set = tokens(theme);
    assert.ok(contrast(set['--term-8'], set['--term-bg']) >= 4.5, `${theme} colour 8`);
    // Two colours of the palette are never the same (black may be white's twin on high contrast's 7 and 15).
    const normal = [1, 2, 3, 4, 5, 6].map((n) => set[`--term-${n}`]);
    assert.equal(new Set(normal).size, 6, `${theme} has six different colours`);
  }
});

test('high contrast has its own set, at 7:1 or more, with black for 0', () => {
  const set = tokens('hc');
  assert.equal(set['--term-bg'], '#000000');
  assert.equal(set['--term-0'], '#000000');
  for (const n of [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]) {
    const ratio = contrast(set[`--term-${n}`], set['--term-bg']);
    assert.ok(ratio >= 7, `hc colour ${n} is ${ratio.toFixed(2)}:1`);
  }
  assert.notDeepEqual(sheet.get('.dk.hc'), sheet.get('.dk.dark'));
});

test('the cursor shows against the background and its character against the cursor', () => {
  for (const theme of Object.keys(THEMES)) {
    const set = tokens(theme);
    assert.ok(contrast(set['--term-cursor'], set['--term-bg']) >= 3, `${theme} cursor against the background`);
    assert.ok(contrast(set['--term-cursor-text'], set['--term-cursor']) >= 4.5, `${theme} cursor text`);
  }
});

test('selected text is readable, and the selection shows against the background', () => {
  for (const theme of Object.keys(THEMES)) {
    const set = tokens(theme);
    assert.ok(contrast(set['--term-sel-fg'], set['--term-sel-bg']) >= 4.5, `${theme} selected text`);
    assert.ok(contrast(set['--term-sel-bg'], set['--term-bg']) >= 3, `${theme} selection against the background`);
  }
});

test('the dark theme keeps the approved board\'s background and foreground', () => {
  const set = tokens('dark');
  assert.equal(set['--term-bg'], '#0A1220');
  assert.equal(set['--term-fg'], '#D7E3F4');
});

test('in forced colours every token is the system\'s', () => {
  const forced = sheet.get('@media (forced-colors: active) .dk, .dk.dark, .dk.hc');
  assert.equal(forced['--term-bg'], 'Canvas');
  assert.equal(forced['--term-fg'], 'CanvasText');
  assert.equal(forced['--term-cursor'], 'CanvasText');
  assert.equal(forced['--term-cursor-text'], 'Canvas');
  assert.equal(forced['--term-sel-bg'], 'Highlight');
  assert.equal(forced['--term-sel-fg'], 'HighlightText');
  for (let n = 0; n < 16; n++) assert.equal(forced[`--term-${n}`], 'CanvasText', `colour ${n}`);
  assert.equal(Object.keys(forced).length, 22);
});

test('the forced-colours rule is as strong as each theme\'s own, and after them', () => {
  const at = css.indexOf('@media (forced-colors: active)');
  assert.ok(at > css.indexOf('.dk.hc {'));
  const selectors = css.slice(at).match(/\.dk[.\w]*/g);
  assert.deepEqual(selectors, ['.dk', '.dk.dark', '.dk.hc']);
});

// What a program may put on the screen: every colour of the theme, and a few of the 256 and true colour.
const OTHERS = ['#000000', '#ffffff', '#808080', '#ff0000', '#00ff00', '#0000ff', '#ffff00', '#5f5f5f', '#d7d7d7'];

test('the contrast module gives the formula\'s values', () => {
  assert.equal(shownContrast('#000000', '#ffffff'), 21);
  assert.ok(Math.abs(shownContrast('#767676', '#ffffff') - contrast('#767676', '#FFFFFF')) < 1e-12);
  assert.equal(MINIMUM, 4.5);
});

test('a colour is read as #rrggbb, and a system colour is no colour', () => {
  assert.equal(parseColour(' #E8C15A'), '#e8c15a');
  assert.equal(parseColour('#abc'), '#aabbcc');
  assert.equal(parseColour('CanvasText'), null);
  assert.equal(parseColour(''), null);
  assert.equal(parseColour(undefined), null);
  assert.equal(parseColour('#12345'), null);
});

test('readable text keeps its colour', () => {
  assert.equal(readable('#d7e3f4', '#0a1220'), '#d7e3f4');
  assert.equal(readable('#000000', '#ffffff'), '#000000');
});

test('every colour a program may choose is readable on every other, in each theme', () => {
  for (const theme of Object.keys(THEMES)) {
    const set = tokens(theme);
    const colours = [...Array.from({ length: 16 }, (_, n) => set[`--term-${n}`]), set['--term-fg'], set['--term-bg'], ...OTHERS].map(parseColour);
    for (const fg of colours) {
      for (const bg of colours) {
        const shown = readable(fg, bg);
        assert.ok(shownContrast(shown, bg) >= MINIMUM, `${theme}: ${fg} on ${bg} drawn as ${shown}`);
      }
    }
  }
});

test('hard-to-read text moves the way it leans, else the other way', () => {
  // The dark theme's bright white on its yellow, as a root prompt's badge: white cannot reach 4.5:1 on that yellow, so it darkens.
  const dark = tokens('dark');
  const onYellow = readable(parseColour(dark['--term-15']), parseColour(dark['--term-3']));
  assert.ok(shownContrast(onYellow, parseColour(dark['--term-3'])) >= 4.5);
  assert.ok(contrast(onYellow.toUpperCase(), '#000000') < contrast(onYellow.toUpperCase(), '#FFFFFF'), `${onYellow} is dark`);
  // Black on the dark theme's background lightens, keeping its hue: a grey-blue, not white.
  const black = readable(parseColour(dark['--term-0']), parseColour(dark['--term-bg']));
  assert.ok(shownContrast(black, parseColour(dark['--term-bg'])) >= 4.5);
  assert.notEqual(black, '#ffffff');
  // It is the nearest: a step less of the way would not do.
  assert.ok(shownContrast(black, parseColour(dark['--term-bg'])) < 4.7, `${black} goes no further than it needs`);
});

test('a contrast past what black or white can give still draws the better of them', () => {
  assert.equal(readable('#777777', '#808080', 30), '#000000');
  assert.equal(readable('#777777', '#202020', 30), '#ffffff');
});
