// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The snapshots by day (design 5.5): every place's snapshots as one list,
// newest first, grouped as Today, Yesterday and Earlier, each with the time
// it was made, the places it covers, what keeps it and a way to browse it.
// Earlier is a line until it is opened. A snapshot of an update and one
// that a backup brought back say so in their own words.

import { fill, h, uniqueId } from '../../dom.js';
import {
  byDay, earlierWords, keptWords, oldestWords, placeShort, placesWords, restoredWords, whyKept,
} from '../../lib/snapshot-model.js';
import { browseHref, dayWords } from '../../lib/snapshot-words.js';
import { bars, failed } from './body.js';

/**
 * Makes the card. `links` says which pages are in this console, `{updates}`.
 * Gives `{element, set}`; `set({rows, settings, from, time, now})` draws the
 * list and leaves the card as it is when what it says has not changed;
 * `rows` are `mergeSnapshots`', `from` the name of the only backup place, or
 * null, for a snapshot that a backup brought back.
 */
export function makeList({ links }) {
  const element = h('section', { class: 'card snap-list', id: 'snapshots', 'aria-labelledby': 'snap-list-h', 'aria-busy': 'true' },
    h('h2', { id: 'snap-list-h' }, 'Snapshots'),
    h('div', { class: 'snap-body' }, bars()));
  let last = null;
  let opened = false;

  /** The time a row is shown by: the clock inside Today and Yesterday, the day and the clock in Earlier. */
  const timeOf = (row, group, { time, now }) => (group === 'earlier'
    ? `${dayWords(row.ms, time.zone, now)} ${time.clock(row.ms / 1000)}`
    : time.clock(row.ms / 1000));

  /** The line under a row: what it holds, or where it came from. */
  function noteOf(row, { time, now, from }) {
    if (row.update) {
      return h('span', { class: 'small muted' }, 'Before the last update: the settings as they were. ',
        links.updates ? h('a', { href: '/updates' }, 'Go back on the Updates page') : ['To go back, run ', h('code', {}, 'elevate sysupdate rollback'), '.']);
    }
    if (row.restored) return h('span', { class: 'small muted' }, restoredWords(row, time.zone, from, now));
    return h('span', { class: 'small muted' }, placesWords(row.places));
  }

  /** One row of a group. */
  function rowOf(row, group, context) {
    const when = timeOf(row, group, context);
    const why = whyKept(row);
    const several = row.places.length > 1;
    const browse = row.places.map((place) => h('a', {
      class: 'btn small',
      href: browseHref(place, row.name, place),
      'aria-label': `Browse ${several ? `${placeShort(place)} in ` : ''}the snapshot of ${group === 'today' ? 'today ' : group === 'yesterday' ? 'yesterday ' : ''}${when}`,
    }, several ? `Browse ${placeShort(place)}` : 'Browse'));
    return h('li', { class: 'snap-row' },
      h('span', { class: 'snap-when' }, h('b', {}, when), why && ` · ${why}`),
      h('span', { class: 'btns' }, browse),
      noteOf(row, context));
  }

  return {
    element,
    /** Says that the snapshots could not be read, `words`, with a button that tries again. */
    fail(words, retry) {
      last = null;
      element.removeAttribute('aria-busy');
      fill(element.querySelector('.snap-body'), failed(words, retry));
    },
    set(model) {
      const groups = byDay(model.rows, model.time.zone, model.now);
      const key = JSON.stringify([groups, model.settings, model.from, model.time.zone]);
      if (key === last) return;
      last = key;
      element.removeAttribute('aria-busy');
      const rule = keptWords(model.settings);
      fill(element.querySelector('h2'), 'Snapshots', rule && h('span', { class: 'more muted' }, rule));
      const body = element.querySelector('.snap-body');
      if (groups.length === 0) {
        fill(body, h('p', { class: 'muted' }, 'No snapshot yet: the first is made within the hour.'));
        return;
      }
      fill(body, ...groups.map((group) => {
        if (group.id !== 'earlier') {
          return h('div', { class: 'snap-group' }, h('h3', { class: 'snap-day' }, group.title), h('ul', { class: 'snap-rows' }, group.rows.map((row) => rowOf(row, group.id, model))));
        }
        const id = uniqueId('snap-earlier');
        // The words are short for the row; the name says which snapshots they open.
        const nameOf = () => `${opened ? 'Hide' : 'Show'} the ${group.rows.length} earlier snapshots`;
        const toggle = h('button', { class: 'btn small', type: 'button', 'aria-expanded': String(opened), 'aria-controls': id, 'aria-label': nameOf() }, opened ? 'Hide' : 'Show all');
        const rows = h('ul', { id, class: 'snap-rows', hidden: !opened }, group.rows.map((row) => rowOf(row, group.id, model)));
        toggle.addEventListener('click', () => {
          opened = !opened;
          rows.hidden = !opened;
          toggle.setAttribute('aria-expanded', String(opened));
          toggle.textContent = opened ? 'Hide' : 'Show all';
          toggle.setAttribute('aria-label', nameOf());
        });
        return h('div', { class: 'snap-group' }, h('h3', { class: 'snap-day' }, group.title),
          h('p', { class: 'snap-earlier' }, h('span', {}, h('b', {}, earlierWords(group.rows))), toggle), rows);
      }), h('p', { class: 'small muted' }, oldestWords(model.rows, model.time.zone, model.now)));
    },
  };
}
