// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Apps and Security cards: each app the check offers an update for, with
// what the update fixes and the buttons to take it now, tonight or put it off
// to later, and the advisories that apply to the system. Taking an update
// that asks for more than the version before stops with the sheet's words,
// which the row shows with "Update anyway" and "Install tonight anyway".

import { fill, h } from '../../dom.js';
import { windowWords } from '../../lib/tonight-model.js';
import {
  button, laterParts, makeCard, paragraphs, taskLine,
} from './cards.js';
import { appRows, appsHead, securityCard } from './model.js';

/** The Apps card. Gives `{element, draw}`. `ctx.notes` holds what a stopped update said, by app. */
export function makeApps(ctx) {
  const card = makeCard('apps', 'Apps');

  /** Runs `action` for the card; one that is not a task reads again. */
  async function act(action, args, one, words) {
    const result = await ctx.run(card, action, args, one, words);
    if (result.task) ctx.begin(result.task, action, args);
    else if (result.ok) await ctx.reload(card.heading);
  }

  const update = (id, one, yes) => act('apps.update', yes ? { id, yes: true } : { id }, one, { unlock: 'update the app', running: 'Updating the app', done: 'The app is updated.' });
  const updateAll = (one) => act('apps.update-all', {}, one, { unlock: 'update every app', running: 'Updating the apps', done: 'The apps are updated.' });
  const tonight = (id, one, yes) => act('apps.tonight', yes ? { id, yes: true } : { id }, one, { unlock: 'install the update tonight', running: 'Asking for the update tonight', done: 'The update installs tonight.' });
  const notTonight = (id, one) => act('apps.not-tonight', { id }, one, { unlock: 'take the update off tonight', done: 'The update does not install tonight.' });
  const tonightAll = (one) => act('apps.tonight-all', {}, one, { unlock: 'install every update tonight', done: 'The updates install tonight.' });

  /** The buttons of a row that is ready: now, tonight (or not tonight) and later, which is `later`'s menu. */
  function actions(row, note, later) {
    return h('div', { class: 'btns' },
      note ? button('Update anyway', (one) => update(row.id, one, true)) : button('Update', (one) => update(row.id, one, false)),
      row.tonight ? button('Not tonight', (one) => notTonight(row.id, one)) : button(note ? 'Install tonight anyway' : 'Tonight', (one) => tonight(row.id, one, Boolean(note))),
      later);
  }

  function draw() {
    const { data, admin } = ctx;
    const rows = appRows(data.check, data.apps, data.flaws, {
      settings: data.settings, now: Date.now(), time: ctx.view.time, names: data.names,
    });
    const head = appsHead(data.check, rows);
    const waiting = rows.filter((row) => row.state === 'update');
    card.head(null, { kind: head.kind, word: head.word });
    fill(card.body,
      paragraphs(head.lines),
      rows.length > 0 && h('ul', { class: 'upd-list' }, ...rows.map((row) => {
        const note = ctx.notes.get(row.id);
        const later = laterParts(ctx, card, row.id, row.name, row.later);
        return h('li', { class: 'upd-row' },
          h('div', {},
            h('b', {}, row.name), ' ', row.pair && h('span', { class: 'mono' }, row.pair), ' ',
            row.state === 'update' && h('span', { class: `pill ${row.kind === 'fixes only' ? 'ok' : 'info'}` }, row.kind),
            ...row.lines.map((line) => h('div', { class: 'muted' }, line)),
            row.tonight && h('div', {}, row.tonight.words),
            later.line,
            note && h('div', { class: 'upd-note' }, note.words),
            note?.sheet && h('pre', { class: 'cmdtext upd-sheet', tabindex: '0', 'aria-label': `What the update of ${row.name} asks for` }, note.sheet)),
          admin && row.state === 'update' && actions(row, note, later.menu));
      })),
      taskLine(ctx.task('apps')) && h('p', { class: 'muted' }, taskLine(ctx.task('apps'))),
      admin && waiting.length > 1 && h('div', { class: 'btns' }, button(`Install ${waiting.length} now`, updateAll, 'default'), waiting.some((row) => !row.tonight) && button('Install tonight', tonightAll)),
      waiting.length > 0 && data.settings && h('p', { class: 'hint' }, windowWords(data.settings.restart)),
      rows.length === 0 && head.kind === 'ok' && h('p', {}, 'Every app is up to date.'));
  }
  return { element: card.element, draw };
}

/** The Security card, and the full advisories read when asked for. Gives `{element, draw}`. */
export function makeSecurity(ctx) {
  const card = makeCard('security', 'Security');
  const full = h('div', { class: 'upd-full' });

  async function show(one) {
    const { showAdvisories } = await import('./advisories.js');
    await showAdvisories(ctx, full, one);
  }

  function draw() {
    const say = securityCard(ctx.data.check);
    card.head(null, null);
    fill(card.body,
      h('p', {}, h('b', {}, say.title)),
      say.rows.length > 0 && h('ul', { class: 'upd-list' }, ...say.rows.map((row) => h('li', {}, row.line, h('div', { class: 'muted' }, row.fix)))),
      ctx.data.check && h('div', { class: 'btns' }, button('Show the advisories', show)),
      full);
  }
  return { element: card.element, draw };
}
