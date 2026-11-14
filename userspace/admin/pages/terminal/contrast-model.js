// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Text a program coloured so that it would be hard to read (design 13.11).
// The theme's colours each have 4.5:1 against the terminal's background,
// but a program may put any colour on any other: a prompt's white on
// yellow, black on the dark theme's background, a 256-colour grey on a
// grey. The screen then draws that text in the colour nearest the
// program's that has 4.5:1 against the cell's background, by the WCAG 2.2
// formula: the program's colour moved towards white or black, the way it
// already leans first. Black or white always reaches 4.5:1 against any
// background, so there is always one.
//
// render.js loads this at a screen's first draw and hands it the runs
// render-model.js makes.

/** The contrast every cell's text has against its background, at least. */
export const MINIMUM = 4.5;

const CACHED = 4096;
const cache = new Map();

/** `text` as '#rrggbb' when it is a '#rgb' or '#rrggbb' colour, else null (a system colour in forced colours, say). */
export function parseColour(text) {
  const value = String(text ?? '').trim().toLowerCase();
  if (/^#[0-9a-f]{6}$/.test(value)) return value;
  if (/^#[0-9a-f]{3}$/.test(value)) return `#${[...value.slice(1)].map((c) => c + c).join('')}`;
  return null;
}

const channelsOf = (hex) => {
  const n = parseInt(hex.slice(1), 16);
  return [n >> 16, (n >> 8) & 255, n & 255];
};
const hexOf = (channels) => `#${channels.map((c) => c.toString(16).padStart(2, '0')).join('')}`;
const linear = (value) => {
  const c = value / 255;
  return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
};
const luminanceOf = ([r, g, b]) => 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
const ratioOf = (a, b) => (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);

/** The WCAG 2.2 contrast of two '#rrggbb' colours, from 1 to 21. */
export function contrast(a, b) {
  return ratioOf(luminanceOf(channelsOf(a)), luminanceOf(channelsOf(b)));
}

// The colour `t` of the way from `from` to `to`, each channel rounded.
const mix = (from, to, t) => from.map((c, i) => Math.round(c + (to[i] - c) * t));

// The colour nearest `fg` on the way to `to` with `minimum` against a
// background of luminance `back`, or null when even `to` has less.
function towards(fg, to, back, minimum) {
  if (ratioOf(luminanceOf(to), back) < minimum) return null;
  let low = 0;
  let high = 1;
  for (let step = 0; step < 16; step++) {
    const t = (low + high) / 2;
    if (ratioOf(luminanceOf(mix(fg, to, t)), back) >= minimum) high = t;
    else low = t;
  }
  return mix(fg, to, high);
}

/**
 * The colour to draw text of colour `fg` on a background `bg` (both
 * '#rrggbb'): `fg` itself when it has `minimum` against `bg`, else the
 * colour nearest it that has.
 */
export function readable(fg, bg, minimum = MINIMUM) {
  const key = `${fg}${bg}${minimum}`;
  const known = cache.get(key);
  if (known !== undefined) return known;
  const front = channelsOf(fg);
  const back = luminanceOf(channelsOf(bg));
  let shown = fg;
  if (ratioOf(luminanceOf(front), back) < minimum) {
    const [first, second] = luminanceOf(front) >= back ? [[255, 255, 255], [0, 0, 0]] : [[0, 0, 0], [255, 255, 255]];
    const best = ratioOf(1, back) >= ratioOf(0, back) ? [255, 255, 255] : [0, 0, 0];
    shown = hexOf(towards(front, first, back, minimum) ?? towards(front, second, back, minimum) ?? best);
  }
  if (cache.size >= CACHED) cache.clear();
  cache.set(key, shown);
  return shown;
}

// A run's classes that keep its colours: dim and invisible text, as the
// program asked for less; the selection and the cursor, which the theme's
// own colours draw.
const KEEP = new Set(['m', 'v', 'sel', 'cur']);
const TEXT = /^f(\d+)$/;
const BACK = /^g(\d+)$/;

// The colour of a run's text (`text` true) or background in `palette`:
// its own, its class's, or the theme's.
function colourOf(run, palette, text) {
  const own = text ? run.color : run.background;
  if (own !== '') return own;
  for (const name of run.cls.split(' ')) {
    const found = (text ? TEXT : BACK).exec(name);
    if (found) return palette.colours[Number(found[1])];
    if (name === (text ? 'fb' : 'bf')) return text ? palette.bg : palette.fg;
  }
  return text ? palette.fg : palette.bg;
}

/**
 * `runs` (render-model.js's `rowRuns`) with readable text, in a theme of
 * `palette` (`{ fg, bg, colours }`, '#rrggbb', the 16 in order): a run
 * whose text has less than MINIMUM against its background gets the nearest
 * colour that has, in place of its own or its class's.
 */
export function readableRuns(runs, palette) {
  return runs.map((run) => {
    const names = run.cls.split(' ');
    const plain = run.cls === '' && run.color === '' && run.background === '';
    if (plain || run.text.trim() === '' || names.some((name) => KEEP.has(name))) return run;
    const text = colourOf(run, palette, true);
    const shown = readable(text, colourOf(run, palette, false));
    if (shown === text) return run;
    return { ...run, cls: names.filter((name) => !TEXT.test(name) && name !== 'fb').join(' '), color: shown };
  });
}

// The theme's colours as term.css gives them to `element`, or null where
// one is not a colour: in forced colours each is a system colour, and the
// program's colours are not drawn.
function paletteOf(element) {
  const style = getComputedStyle(element);
  const read = (name) => parseColour(style.getPropertyValue(name));
  const colours = Array.from({ length: 16 }, (_, n) => read(`--term-${n}`));
  const palette = { fg: read('--term-fg'), bg: read('--term-bg'), colours };
  return palette.fg === null || palette.bg === null || colours.includes(null) ? null : palette;
}

/**
 * Starts drawing hard-to-read text readable. `redraw` is called now and
 * each time the theme changes, which the root's classes say (or forced
 * colours, which a media query says). The function returned takes a
 * screen's runs and its element, and gives the runs to draw.
 */
export function follow(redraw) {
  let palettes = new WeakMap();
  const again = () => {
    palettes = new WeakMap();
    redraw();
  };
  new MutationObserver(again).observe(document.documentElement, { attributes: true, attributeFilter: ['class'] });
  matchMedia('(forced-colors: active)').addEventListener('change', again);
  again();
  return (runs, element) => {
    if (!palettes.has(element)) palettes.set(element, paletteOf(element));
    const palette = palettes.get(element);
    return palette === null ? runs : readableRuns(runs, palette);
  };
}
