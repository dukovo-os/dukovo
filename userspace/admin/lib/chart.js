// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// One line chart of the Usage card (design 10.3), drawn as SVG with
// `createElementNS`: a big number and its note above, the picture, a legend
// when it has two lines, and "Show the numbers", which opens the points as a
// table. The picture is `role="img"` with a label in words, since the lines
// alone say nothing to a screen reader. What the chart shows, and the words,
// are lib/chart-model.js's. With `pick`, a person can also choose a moment on
// it (design 6.3): a slider over the picture, which lib/chart-pick-model.js
// moves, and the marks of the journal under it.

import { h, uniqueId } from '../dom.js';
import {
  labelOf, shapes, statOf, tableOf, VIEW, xOf,
} from './chart-model.js';
import {
  indexOfTime, markPlaces, PICK_HINT, pickAt, pickLabel, pickWords, sliderName, step, timeAt, timesOf,
} from './chart-pick-model.js';

const SVG = 'http://www.w3.org/2000/svg';

/** An SVG element with `attributes`; its children are SVG elements. */
function svg(tag, attributes = {}, ...children) {
  const element = document.createElementNS(SVG, tag);
  for (const [name, value] of Object.entries(attributes)) element.setAttribute(name, value);
  element.append(...children);
  return element;
}

/**
 * The part of a chart that chooses a moment: the slider over the picture, the
 * line and the label that follow a pointer or a key, the line at the moment
 * chosen, the marks of the journal as triangles and as buttons. `choose(at)`
 * is told the time (Unix seconds) that a click, Enter or a mark's button
 * chooses. Its `update(model, time)` is called after the picture is drawn.
 */
function makePicker(id, picture, choose, first) {
  const hintId = uniqueId(`pick-hint-${id}`);
  const hint = h('p', { id: hintId, class: 'small muted pick-hint', hidden: true }, PICK_HINT);
  const layer = h('div', {
    class: 'pick-layer', role: 'slider', tabindex: '0', 'aria-orientation': 'horizontal', 'aria-describedby': hintId, hidden: true,
  });
  const label = h('span', { class: 'pick-label', 'aria-hidden': 'true', hidden: true });
  const frame = h('div', { class: 'chart-frame' }, picture, layer, label);
  const overlay = svg('g', { class: 'overlay' });
  const list = h('ul', { class: 'marks', 'aria-label': 'Changes on this chart' });
  const listBox = h('div', { class: 'marks-box', hidden: true }, h('p', { class: 'small muted' }, 'Changes on this chart. Choose one to look at that moment.'), list);

  let model = null;
  let time = null;
  let marks = first;
  // The point a key last moved to (null until one has), the one a pointer is over, the time chosen and whether the slider has focus.
  let index = null;
  let hover = null;
  let chosen = null;
  let focused = false;

  const lastIndex = () => timesOf(model).length - 1;

  /** The point whose line and label are shown: the pointer's, else the slider's while it has focus. */
  const cursorIndex = () => hover ?? (focused ? (index ?? lastIndex()) : null);

  function drawOverlay() {
    const parts = [];
    for (const mark of markPlaces(model, marks)) parts.push(svg('polygon', { class: 'pick', points: `${mark.x - 2.5},0 ${mark.x + 2.5},0 ${mark.x},5` }));
    if (chosen !== null) parts.push(svg('line', { class: 'mark', x1: xOf(model, chosen), y1: 0, x2: xOf(model, chosen), y2: VIEW.height }));
    const at = cursorIndex();
    const place = at === null ? null : timeAt(model, at);
    if (place !== null) {
      const x = xOf(model, place);
      parts.push(svg('line', { class: 'cursor', x1: x, y1: 0, x2: x, y2: VIEW.height }));
      label.textContent = pickLabel(model, at, time);
      // Held away from the edges, so that the words stay inside the picture.
      label.style.left = `${Math.min(88, Math.max(12, (x / VIEW.width) * 100))}%`;
    }
    label.hidden = place === null;
    overlay.replaceChildren(...parts);
  }

  function describe() {
    const at = index ?? lastIndex();
    layer.setAttribute('aria-valuemin', '0');
    layer.setAttribute('aria-valuemax', String(lastIndex()));
    layer.setAttribute('aria-valuenow', String(at));
    layer.setAttribute('aria-valuetext', pickWords(model, at, time));
  }

  function drawMarks() {
    const placed = markPlaces(model, marks);
    list.replaceChildren(...placed.map((mark) => h('li', {},
      h('button', { class: 'btn link small', type: 'button', 'data-at': String(mark.at) }, `${time.when(mark.at)} ${mark.label}`))));
    listBox.hidden = placed.length === 0;
  }

  /** Chooses the time `at`: the slider goes to the point nearest to it, and the page is told. */
  function take(at) {
    chosen = at;
    index = indexOfTime(model, at);
    describe();
    drawOverlay();
    choose(at);
  }

  layer.addEventListener('pointermove', (event) => {
    const box = layer.getBoundingClientRect();
    if (box.width === 0) return;
    hover = pickAt(model, ((event.clientX - box.left) / box.width) * VIEW.width);
    drawOverlay();
  });
  layer.addEventListener('pointerleave', () => {
    hover = null;
    drawOverlay();
  });
  layer.addEventListener('click', (event) => {
    const box = layer.getBoundingClientRect();
    // A tap has no move before it, so the point is read from the click.
    const point = box.width === 0 ? null : pickAt(model, ((event.clientX - box.left) / box.width) * VIEW.width);
    if (point !== null) take(timeAt(model, point));
  });
  layer.addEventListener('keydown', (event) => {
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    if (event.key === 'Enter') {
      event.preventDefault();
      take(timeAt(model, index ?? lastIndex()));
      return;
    }
    const next = step(model, index, event.key);
    if (next === null) return;
    event.preventDefault();
    index = next;
    describe();
    drawOverlay();
  });
  layer.addEventListener('focus', () => {
    focused = true;
    drawOverlay();
  });
  layer.addEventListener('blur', () => {
    focused = false;
    drawOverlay();
  });
  list.addEventListener('click', (event) => {
    const button = event.target.closest('button[data-at]');
    if (button) take(Number(button.dataset.at));
  });

  return {
    frame,
    hint,
    marks: listBox,
    update(next, now) {
      model = next;
      time = now;
      const points = lastIndex() >= 0;
      layer.hidden = !points;
      hint.hidden = !points;
      if (!points) {
        index = null;
        hover = null;
      } else {
        if (index !== null) index = Math.min(index, lastIndex());
        layer.setAttribute('aria-label', sliderName(model));
        describe();
      }
      picture.append(overlay);
      drawOverlay();
      drawMarks();
    },
    /** The marks to draw, `[{at, label}]` with `at` in Unix seconds, as the journal's rows are. */
    setMarks(next) {
      marks = next;
      if (model) {
        drawOverlay();
        drawMarks();
      }
    },
    /** The moment chosen, in Unix seconds, or null for none: the line is drawn at it, and the slider is moved to the point nearest to it. */
    setPicked(at) {
      chosen = at;
      if (model && at !== null && lastIndex() >= 0) {
        index = indexOfTime(model, at);
        describe();
      }
      if (model) drawOverlay();
    },
  };
}

/**
 * A chart that is drawn again with `update(model, time)` each time its numbers
 * are read, so that "Show the numbers" stays open while they change. `id` is
 * the chart's own (`cpu`); gives `{element, update}`. With `pick`, a function
 * called with the time (Unix seconds) that a person chooses, the chart can be
 * chosen from; `marks` is the journal's rows `[{at, label}]` to mark on it,
 * and it also gives `setMarks(list)` for rows that come later and
 * `setPicked(at)` for a moment already chosen (null for none). Without
 * `pick` the chart is the Usage card's.
 */
export function makeChart(id, { pick = null, marks = [] } = {}) {
  const tableId = uniqueId(`numbers-${id}`);
  const big = h('b', {});
  const note = h('span', {});
  // An image is named in words, so it is one only once update() has its words.
  const picture = svg('svg', { class: 'chart', viewBox: `0 0 ${VIEW.width} ${VIEW.height}`, preserveAspectRatio: 'none', 'aria-hidden': 'true' });
  const legend = h('ul', { class: 'legend' });
  const table = h('table', { class: 'tbl' });
  const box = h('div', { id: tableId, class: 'tbl-box numbers', role: 'region', tabindex: '0', hidden: true }, table);
  const toggle = h('button', { class: 'btn link small', type: 'button', 'aria-expanded': 'false', 'aria-controls': tableId }, 'Show the numbers');
  toggle.addEventListener('click', () => {
    const open = toggle.getAttribute('aria-expanded') !== 'true';
    toggle.setAttribute('aria-expanded', String(open));
    toggle.textContent = open ? 'Hide the numbers' : 'Show the numbers';
    box.hidden = !open;
  });

  const picker = pick ? makePicker(id, picture, pick, marks) : null;
  const element = h('div', { class: 'usage-chart', 'data-chart': id },
    h('div', { class: 'stat' }, big, note), picker ? picker.frame : picture, ...(picker ? [picker.hint] : []), legend, ...(picker ? [picker.marks] : []), toggle, box);

  function update(model, time) {
    const words = statOf(model, time);
    big.textContent = words.value;
    note.textContent = words.note;
    const label = labelOf(model, time);
    picture.removeAttribute('aria-hidden');
    picture.setAttribute('role', 'img');
    picture.setAttribute('aria-label', label);

    const [first, ...others] = model.lines;
    const under = shapes(model, first);
    picture.replaceChildren(
      svg('line', { class: 'grid-line', x1: 0, y1: VIEW.height / 2, x2: VIEW.width, y2: VIEW.height / 2 }),
      // The first line has the fill under it; the others are lines only, so that two fills do not hide each other.
      ...under.map((shape) => svg('polygon', { class: 'area', points: shape.area })),
      ...under.map((shape) => svg('polyline', { class: 'line', points: shape.points })),
      ...others.flatMap((line) => shapes(model, line).map((shape) => svg('polyline', { class: 'line second', points: shape.points }))));

    legend.replaceChildren(...(model.lines.length > 1
      ? model.lines.map((line, at) => h('li', {}, h('span', { class: at === 0 ? 'key' : 'key second', 'aria-hidden': 'true' }), `${line.label}${at === 0 ? '' : ' (dashed)'}`))
      : []));
    legend.hidden = model.lines.length < 2;

    box.setAttribute('aria-label', `Numbers of ${model.chart.name} over ${model.range.words}`);
    const { head, rows } = tableOf(model, time);
    table.replaceChildren(
      h('caption', { class: 'sr-only' }, `${model.chart.name}, ${model.range.words}`),
      h('thead', {}, h('tr', {}, ...head.map((words, at) => h('th', { scope: 'col', class: at === 0 ? '' : 'num' }, words)))),
      h('tbody', {}, ...rows.map((row) => h('tr', {}, ...row.map((cell, at) => h(at === 0 ? 'th' : 'td', at === 0 ? { scope: 'row' } : { class: 'num' }, cell))))));
    toggle.hidden = model.empty;
    if (model.empty) box.hidden = true;
    picker?.update(model, time);
  }

  return picker ? { element, update, setMarks: picker.setMarks, setPicked: picker.setPicked } : { element, update };
}
