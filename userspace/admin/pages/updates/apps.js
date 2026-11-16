// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Apps and Security cards: each app the check offers an update for, with
// what the update fixes and a button to take it, and the advisories that
// apply to the system. Taking an update that asks for more than the version
// before stops with the sheet's words, which the row shows with "Update
// anyway".

import { fill, h } from '../../dom.js';
import { button, makeCard, paragraphs, taskLine } from './cards.js';
import { appRows, appsHead, securityCard } from './model.js';

/** The Apps card. Gives `{element, draw}`. `ctx.notes` holds what a stopped update said, by app. */
export function makeApps(ctx) {
  const card = makeCard('apps', 'Apps');

  async function update(id, one, yes) {
    const args = yes ? { id, yes: true } : { id };
    const result = await ctx.run(card, 'apps.update', args, one, { unlock: 'update the app', running: 'Updating the app', done: 'The app is updated.' });
    if (result.task) ctx.begin(result.task, 'apps.update', args);
    else if (result.ok) await ctx.reload(card.heading);
  }

  async function updateAll(one) {
    const result = await ctx.run(card, 'apps.update-all', {}, one, { unlock: 'update every app', running: 'Updating the apps', done: 'The apps are updated.' });
    if (result.task) ctx.begin(result.task, 'apps.update-all', {});
  }

  function draw() {
    const { data, admin } = ctx;
    const rows = appRows(data.check, data.apps, data.flaws);
    const head = appsHead(data.check, rows);
    const waiting = rows.filter((row) => row.state === 'update');
    card.head(null, { kind: head.kind, word: head.word });
    fill(card.body,
      paragraphs(head.lines),
      rows.length > 0 && h('ul', { class: 'upd-list' }, ...rows.map((row) => h('li', { class: 'upd-row' },
        h('div', {},
          h('b', {}, row.name), ' ', row.pair && h('span', { class: 'mono' }, row.pair), ' ',
          row.state === 'update' && h('span', { class: `pill ${row.kind === 'fixes only' ? 'ok' : 'info'}` }, row.kind),
          ...row.lines.map((line) => h('div', { class: 'muted' }, line)),
          ctx.notes.has(row.id) && h('div', { class: 'upd-note' }, ctx.notes.get(row.id))),
        admin && row.state === 'update' && (ctx.notes.has(row.id)
          ? button('Update anyway', (one) => update(row.id, one, true))
          : button('Update', (one) => update(row.id, one, false)))))),
      taskLine(ctx.task('apps')) && h('p', { class: 'muted' }, taskLine(ctx.task('apps'))),
      admin && waiting.length > 1 && h('div', { class: 'btns' }, button(`Install ${waiting.length} now`, updateAll, 'default')),
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
