// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Where alerts go: a row for each channel with what it is told of and how it
// last did, the lines of the last test, and the buttons to edit a channel's
// kinds, remove it or add a webhook. The questions are read when asked.

import { fill, h, icon } from '../../dom.js';
import { card, nothing } from './cards.js';
import { listOf } from './channels-model.js';
import {
  channelRows, NO_CHANNEL_STATE, SCREENS_WORDS,
} from './model.js';
import { uncoveredWords } from './told-model.js';

/** The Where alerts go card. `ctx` is the page's: `{view, reads, load, test, pushId}`. */
export function makeChannels(ctx) {
  const where = card('channels', 'Where alerts go');
  const add = h('button', { class: 'btn small', type: 'button' }, 'Add a webhook…');
  where.tools.append(add);
  const results = h('ul', { class: 'al-test', role: 'status' });
  const warning = h('div', { class: 'banner warn', hidden: true });
  const list = h('ul', { class: 'al-list' });
  fill(where.body, results, warning, list);
  let rows = [];

  const ask = async (module, row) => {
    const dialogs = await import('./row-dialogs.js');
    if (await dialogs[module](ctx.view, row)) {
      if (module === 'openRemove' && row.here) ctx.removed(row.channel);
      ctx.load();
    }
  };
  add.addEventListener('click', async () => {
    const { openWebhook } = await import('./webhook.js');
    if (await openWebhook(ctx.view)) ctx.load();
  });

  const rowView = (row) => {
    const edit = row.arg ? h('button', { class: 'btn small', type: 'button', onclick: () => ask('openTold', row) }, 'Edit what it is told') : null;
    const test = row.bad ? h('button', { class: 'btn small', type: 'button' }, 'Send a test alert') : null;
    test?.addEventListener('click', () => ctx.test(test));
    return h('li', { class: `al-item${row.bad ? ' bad' : ''}` },
      h('div', {},
        h('b', {}, row.name),
        row.here && h('span', { class: 'pill info' }, 'This browser'),
        h('p', { class: 'al-told' }, row.told),
        row.sent && h('p', { class: 'muted small' }, row.sent),
        row.failed && h('p', { class: 'al-bad small' }, icon('warn'), h('span', {}, row.failed, row.id && h('span', { class: 'mono' }, ` ${row.id}`))),
        row.none && h('p', { class: 'muted small' }, 'Nothing has been sent yet.'),
        row.waiting && h('p', { class: 'muted small' }, row.waiting)),
      h('div', { class: 'btns' }, edit, test, h('button', { class: 'btn small', type: 'button', onclick: () => ask('openRemove', row) }, 'Remove')));
  };

  return {
    element: where.element,
    /** The channels in `result`, a read of the page's. */
    set(result) {
      if (result.state !== 'ok') {
        fill(list, h('li', {}, ctx.reads.note(result, 'where alerts go', ctx.load)));
        return;
      }
      const channels = listOf(result.value);
      rows = channelRows(channels, ctx.view.time, ctx.pushId());
      const left = uncoveredWords(channels);
      const email = rows.find((row) => row.kind === 'email');
      fill(warning, left && [icon('warn'), h('div', {}, h('p', {}, left), email && h('div', { class: 'btns' }, h('button', { class: 'btn small', type: 'button', onclick: () => ask('openTold', email) }, 'Edit what email is told')))]);
      warning.hidden = left === null;
      fill(list,
        h('li', { class: 'al-item' }, h('div', {}, h('b', {}, SCREENS_WORDS))),
        ...rows.map(rowView),
        channels.length === 0 && h('li', {}, nothing(NO_CHANNEL_STATE)));
    },
    /** Shows the lines of a test: `[{ok, words, id}]`, or `words` alone when there is none to show. */
    showTest(lines) {
      fill(results, typeof lines === 'string'
        ? h('li', {}, lines)
        : lines.map((line) => h('li', { class: line.ok ? 'al-ok' : 'al-bad' }, icon(line.ok ? 'check' : 'warn'), h('span', {}, line.words, line.id && h('span', { class: 'mono' }, ` ${line.id}`)))));
    },
    rows: () => rows,
  };
}
