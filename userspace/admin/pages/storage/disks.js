// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The disks, drawn to scale (design 4.3): a row for each disk with its name,
// its model and size, a bar whose parts are its partitions at their share,
// each labelled with what it is for, and the pill of its health. The name is
// a link to the page of the disk; the whole row takes the click. A bar is an
// image with its words, and the partitions are a list beside it for a screen
// reader; a part too narrow for words has a number, and a key under the bar
// says what each number is.

import { fill, h } from '../../dom.js';
import { bar } from './use.js';

/** One part of a bar. Its width is its share of the disk, as a flex ratio. */
function partOf(part) {
  const span = h('span', { class: part.kind, title: part.label }, part.number === null ? part.label : String(part.number));
  span.style.setProperty('flex-grow', part.percent.toFixed(3));
  return span;
}

/** One row. `selected` is true for the disk whose page is open; `backups` is true when the page of backup places exists, for an empty disk's link. */
function rowOf(row, { selected, backups }) {
  const link = h('a', { class: 'sto-disk-link', href: row.href, 'aria-current': selected ? 'page' : null }, row.name);
  return h('li', { class: `disk${selected ? ' sel' : ''}`, 'data-disk': row.name },
    h('div', { class: 'sto-disk-name' }, h('b', {}, link), h('span', { class: 'small muted' }, row.line),
      row.empty && backups && h('a', { class: 'small sto-backups', href: '/snapshots/backups' }, 'Use it for backups')),
    h('div', { class: 'sto-map' },
      h('div', { class: 'dmap', role: 'img', 'aria-label': row.label }, row.parts.map(partOf)),
      row.key.length > 0 && h('p', { class: 'small muted sto-key', 'aria-hidden': 'true' }, row.key.join(' · ')),
      h('ul', { class: 'sr-only' },
        row.parts.map((part) => h('li', {}, part.label)),
        h('li', {}, `${row.name}: ${row.health.word}. ${row.health.note}`.trim()))),
    h('span', { class: `st ${row.health.tone}`, title: row.health.note }, row.health.word));
}

/**
 * The card of disks. `set(rows, selected)` draws the rows of `rowsOf` and
 * marks the disk named `selected`; `frame()` draws the grey bars. Gives
 * `{element, set, frame}`.
 */
export function makeDisks({ backups }) {
  const body = h('ul', { class: 'sto-disks' });
  const element = h('section', { class: 'card sto-disks-card', id: 'disks', 'aria-labelledby': 'sto-disks-h' },
    h('h2', { id: 'sto-disks-h' }, 'Disks, drawn to scale'), body);
  let last = null;
  return {
    element,
    frame() {
      fill(body, [0, 1].map(() => h('li', { class: 'disk', 'aria-hidden': 'true' }, bar(), bar(), bar())));
    },
    set(rows, selected) {
      const key = JSON.stringify([rows, selected]);
      if (key === last) return;
      last = key;
      const held = body.contains(document.activeElement) ? document.activeElement.closest('[data-disk]')?.dataset.disk : null;
      fill(body, rows.map((row) => rowOf(row, { selected: row.name === selected, backups })));
      if (held) body.querySelector(`[data-disk="${CSS.escape(held)}"] a`)?.focus();
    },
  };
}
