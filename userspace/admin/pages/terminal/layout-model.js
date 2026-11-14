// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// How a terminal's panes are laid out, as plain values (design 13.10): the
// rectangle of each pane as fractions of the box, the separators between
// them, and the moves the keys make. admind keeps a terminal's `layout`
// (one, columns, rows, main-left or grid) and its `sizes`: each size is the
// share a split gives its first side, 0.2 to 0.8, so a window never ends
// up with a pane too small to type in.
//
//   columns    pane 0 | pane 1 | ...   size i splits what pane i and the rest share
//   rows       the same, one above the other
//   main-left  pane 0 on the left (size 0), the rest stacked at its right (sizes 1 on)
//   grid       two columns of two rows: size 0 splits left from right, size 1 top from bottom
//   one        one pane at a time: the focused one

import { LAYOUTS, LAYOUT_WORDS } from './workspace-model.js';

export { LAYOUTS, LAYOUT_WORDS };
export const SIZE_MIN = 0.2;
export const SIZE_MAX = 0.8;
export const MOST_SIZES = 3;
export const PANES_MOST = 4;

const clamp = (size) => Math.min(Math.max(size, SIZE_MIN), SIZE_MAX);

/** The number of splits (and so of sizes it uses) a layout has for `count` panes. */
export function splitsOf(layout, count) {
  if (count < 2 || layout === 'one') return 0;
  if (layout === 'grid') return count === 2 ? 1 : 2;
  return count - 1;
}

/** The sizes admind keeps, made as many as the layout has splits: a missing one is an equal share. */
export function sizesFor(layout, count, sizes = []) {
  const splits = splitsOf(layout, count);
  const own = Array.isArray(sizes) ? sizes : [];
  const out = [];
  for (let i = 0; i < splits; i++) {
    const equal = layout === 'columns' || layout === 'rows' ? 1 / (count - i) : 0.5;
    out.push(clamp(typeof own[i] === 'number' && Number.isFinite(own[i]) ? own[i] : equal));
  }
  return out;
}

// A rectangle in fractions of the box.
const rect = (x, y, w, h) => ({ x, y, w, h });

// Splits `box` along `axis` in the share `size` for the first side.
function split(box, axis, size) {
  const cut = axis === 'x' ? box.w * size : box.h * size;
  return axis === 'x'
    ? [rect(box.x, box.y, cut, box.h), rect(box.x + cut, box.y, box.w - cut, box.h)]
    : [rect(box.x, box.y, box.w, cut), rect(box.x, box.y + cut, box.w, box.h - cut)];
}

// A separator between two sides of `box`, which it moves within.
const separator = (box, axis, size, index) => ({
  index, axis, size, from: axis === 'x' ? box.x : box.y, to: axis === 'x' ? box.x + box.w : box.y + box.h,
  at: axis === 'x' ? box.x + box.w * size : box.y + box.h * size,
  across: axis === 'x' ? [box.y, box.y + box.h] : [box.x, box.x + box.w],
});

// The panes in a line along `axis`, each taking its share of what is left.
function line(box, axis, count, sizes, first, panes, separators) {
  let rest = box;
  for (let i = 0; i < count - 1; i++) {
    const size = sizes[first + i];
    const [own, next] = split(rest, axis, size);
    panes.push(own);
    separators.push(separator(rest, axis, size, first + i));
    rest = next;
  }
  panes.push(rest);
}

/**
 * Where each pane goes: `{ panes: [{x, y, w, h}], separators: [{index,
 * axis, size, from, to, at, across}] }`, in fractions of the box, panes in
 * order. `separators[i].index` is the size it moves; `from` and `to` are
 * the ends it may move between (it keeps 0.2 to 0.8 of them). `focus` is
 * the pane that shows alone in layout `one`.
 */
export function placeOf(layout, count, sizes = [], focus = 0) {
  const kept = sizesFor(layout, count, sizes);
  const whole = rect(0, 0, 1, 1);
  const panes = [];
  const separators = [];
  if (count < 2 || layout === 'one') {
    return { panes: Array.from({ length: count }, (_, i) => (i === focus || count < 2 ? whole : null)), separators };
  }
  if (layout === 'columns' || layout === 'rows') {
    line(whole, layout === 'columns' ? 'x' : 'y', count, kept, 0, panes, separators);
  } else if (layout === 'main-left') {
    const [left, right] = split(whole, 'x', kept[0]);
    panes.push(left);
    separators.push(separator(whole, 'x', kept[0], 0));
    line(right, 'y', count - 1, kept, 1, panes, separators);
  } else if (count === 2) {
    const [left, right] = split(whole, 'x', kept[0]);
    panes.push(left, right);
    separators.push(separator(whole, 'x', kept[0], 0));
  } else {
    const [top, bottom] = split(whole, 'y', kept[1]);
    separators.push(separator(whole, 'y', kept[1], 1));
    if (count === 3) {
      const [a, b] = split(top, 'x', kept[0]);
      panes.push(a, b, bottom);
      separators.push(separator(top, 'x', kept[0], 0));
    } else {
      const [a, b] = split(top, 'x', kept[0]);
      const [c, d] = split(bottom, 'x', kept[0]);
      panes.push(a, b, c, d);
      // One line down the whole box: both rows have the same split.
      separators.push(separator(whole, 'x', kept[0], 0));
    }
  }
  return { panes, separators };
}

/** The sizes with size `index` set to `size` (kept within 0.2 to 0.8), or moved by `size` when `by` is true. */
export function movedSizes(layout, count, sizes, index, size, by = false) {
  const kept = sizesFor(layout, count, sizes);
  if (index < 0 || index >= kept.length) return kept;
  kept[index] = Math.round(clamp(by ? kept[index] + size : size) * 1000) / 1000;
  return kept;
}

/** The size a pointer at `position` (a fraction of the box) asks of `separator`. */
export function sizeAt(separator, position) {
  return (position - separator.from) / (separator.to - separator.from);
}

// How much of two ranges lie together.
const overlap = (a0, a1, b0, b1) => Math.max(0, Math.min(a1, b1) - Math.max(a0, b0));
const EDGE = 1e-6;

/**
 * The pane next to pane `from` in `direction` ('left', 'right', 'up' or
 * 'down'): the pane that touches its edge there and shares the most of it,
 * or null at the edge of the box (a pane that is not shown does not count).
 */
export function neighbour(places, from, direction) {
  const here = places[from];
  if (!here) return null;
  let best = null;
  let most = 0;
  places.forEach((other, index) => {
    if (!other || index === from) return;
    const touches = {
      left: Math.abs(other.x + other.w - here.x) < EDGE,
      right: Math.abs(other.x - (here.x + here.w)) < EDGE,
      up: Math.abs(other.y + other.h - here.y) < EDGE,
      down: Math.abs(other.y - (here.y + here.h)) < EDGE,
    }[direction];
    if (!touches) return;
    const shared = direction === 'left' || direction === 'right'
      ? overlap(here.y, here.y + here.h, other.y, other.y + other.h)
      : overlap(here.x, here.x + here.w, other.x, other.x + other.w);
    if (shared > most + EDGE) {
      best = index;
      most = shared;
    }
  });
  return best;
}

/**
 * The layout for a terminal that gets one more pane by splitting in
 * `direction` ('right' or 'down'). One pane becomes two columns, or two
 * rows. With more, a split along the line the panes already lie in keeps
 * the layout; across it makes a grid; main-left stays.
 */
export function layoutForSplit(layout, count, direction) {
  const wanted = direction === 'right' ? 'columns' : 'rows';
  if (count < 2 || layout === 'one') return wanted;
  if (layout === wanted || layout === 'main-left' || layout === 'grid') return layout;
  return 'grid';
}
