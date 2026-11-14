// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// One line chart of the Usage card (design 10.3), drawn as SVG with
// `createElementNS`: a big number and its note above, the picture, a legend
// when it has two lines, and "Show the numbers", which opens the points as a
// table. The picture is `role="img"` with a label in words, since the lines
// alone say nothing to a screen reader. What the chart shows, and the words,
// are lib/chart-model.js's.

import { h, uniqueId } from '../dom.js';
import {
  labelOf, shapes, statOf, tableOf, VIEW,
} from './chart-model.js';

const SVG = 'http://www.w3.org/2000/svg';

/** An SVG element with `attributes`; its children are SVG elements. */
function svg(tag, attributes = {}, ...children) {
  const element = document.createElementNS(SVG, tag);
  for (const [name, value] of Object.entries(attributes)) element.setAttribute(name, value);
  element.append(...children);
  return element;
}

/**
 * A chart that is drawn again with `update(model, time)` each time its numbers
 * are read, so that "Show the numbers" stays open while they change. `id` is
 * the chart's own (`cpu`); gives `{element, update}`.
 */
export function makeChart(id) {
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

  const element = h('div', { class: 'usage-chart', 'data-chart': id },
    h('div', { class: 'stat' }, big, note), picture, legend, toggle, box);

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
  }

  return { element, update };
}
