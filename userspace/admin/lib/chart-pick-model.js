// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Choosing a moment on a chart (design 6.3): which point a key or a pointer
// picks, the words of a point, and where the journal's marks fall. A point is
// a time at which any line of the chart has a value, counted from the oldest,
// so that the slider lib/chart.js draws has a first and a last. Pure.

import { FORMATS, VIEW, xOf } from './chart-model.js';

/** What the hint under a chart that can be picked from says. */
export const PICK_HINT = 'Arrow keys move along the chart; Enter chooses a moment.';

/** The name of the slider of `model`: "Choose a moment on the CPU chart". */
export const sliderName = (model) => `Choose a moment on the ${model.chart.name} chart`;

/** The times of the points of `model`, oldest first, each once: a time at which any line has a value. */
export function timesOf(model) {
  return [...new Set(model.lines.flatMap((line) => line.points.map((point) => point.at)))].sort((a, b) => a - b);
}

/** The index in `times` of the one nearest to `want`; the earlier of two that are as near; null when there are none. */
function nearest(times, want) {
  if (times.length === 0 || !Number.isFinite(want)) return null;
  let best = 0;
  for (let index = 1; index < times.length; index += 1) {
    if (Math.abs(times[index] - want) < Math.abs(times[best] - want)) best = index;
  }
  return best;
}

/** The point nearest to the time `at` (Unix seconds), or null when the chart has none. */
export const indexOfTime = (model, at) => nearest(timesOf(model), at);

/**
 * The point nearest to `x`, a place in the picture's own units (0 to
 * `VIEW.width`, which a pointer's place across the picture is scaled to);
 * one beyond either end is that end's. Null when the chart has no point.
 */
export function pickAt(model, x) {
  return nearest(timesOf(model), model.from + (x / VIEW.width) * (model.to - model.from));
}

// A slider's keys: an arrow moves a point, a page ten, and later is up, as in a slider's own keys.
const MOVES = {
  ArrowLeft: -1, ArrowDown: -1, ArrowRight: 1, ArrowUp: 1, PageDown: -10, PageUp: 10,
};

/**
 * The point that `key` moves to from `index` (the newest when nothing is
 * picked yet), held between the first and the last: a left or down arrow one
 * point back, a right or up arrow one forward, Page Down and Page Up ten,
 * Home the first and End the last. Null for another key, and when the chart
 * has no point, so that the page leaves the key alone.
 */
export function step(model, index, key) {
  const last = timesOf(model).length - 1;
  if (last < 0) return null;
  if (key === 'Home') return 0;
  if (key === 'End') return last;
  if (!Object.hasOwn(MOVES, key)) return null;
  return Math.min(last, Math.max(0, (index ?? last) + MOVES[key]));
}

/** The value of `line` at `at`, in the chart's own words, or "no number". */
function valueWords(model, line, at) {
  const point = line.points.find((one) => one.at === at);
  return point ? FORMATS[model.chart.kind](point.value) : 'no number';
}

/**
 * The words of the point `index` as a picture shows them: "2:05 PM, 63%", or
 * for a chart of two lines "2:05 PM, read 1.2 MB/s, written 300 KB/s". Empty
 * when there is no such point. `time` is lib/time.js's.
 */
export function pickLabel(model, index, time) {
  const at = timesOf(model)[index];
  if (at === undefined) return '';
  const values = model.lines.length > 1
    ? model.lines.map((line) => `${line.label.toLowerCase()} ${valueWords(model, line, at)}`)
    : [valueWords(model, model.lines[0], at)];
  return `${time.when(at)}, ${values.join(', ')}`;
}

/** The same words for a screen reader, which is told "63 percent" and not read a sign. */
export const pickWords = (model, index, time) => pickLabel(model, index, time).replace(/(\d)%/g, '$1 percent');

/** The time, in Unix seconds, of the point `index`, or null. */
export const timeAt = (model, index) => timesOf(model)[index] ?? null;

/**
 * Where the marks fall on the picture: `[{at, label, x}]`, oldest first, for
 * the ones `{at, label}` that are inside the chart's range, with `x` in the
 * picture's units. A mark outside the range is left out, so that the chart
 * and the list under it say the same.
 */
export function markPlaces(model, marks) {
  return marks
    .filter((mark) => mark.at >= model.from && mark.at <= model.to)
    .map((mark) => ({ ...mark, x: xOf(model, mark.at) }))
    .sort((a, b) => a.at - b.at);
}
