// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What they did, and what no client can do. The first is the clients' own
// history, which admind keeps: each change asked for with what became of
// it, and each client's reads joined into runs. The second is the four
// rules that hold whatever a client was given, so that a person who adds
// one knows what cannot happen.

import { fill, h, icon } from '../../dom.js';
import { historyLines } from './model.js';
import { plainCommand, section, unread } from './parts.js';

/** How many lines the card shows; the log has every call. */
const SHOWN = 12;

const NONE = 'No client has done anything yet.';
const LOGGED = 'Every call a client makes is one line in the log, with its name.';
const LOG_COMMAND = 'elevate log --service admind --grep "AI client"';

/** What holds for every client, whatever it was given: a few words in bold, and the rest. */
const NEVER = [
  ['Change anything without a yes.', 'An administrator with the console unlocked approves each change, and it runs as that administrator.'],
  ['See a secret.', 'Passwords, keys and tokens are never shown to a client, and it cannot ask for one to be made or shown.'],
  ['Change who gets in.', 'Accounts, sign-in rules, SSH, the private network and this page can be read by a client, never changed.'],
  ['Outlast its end date.', 'A client always ends, a year after it was added at most.'],
];

/** The What they did card. `ctx` is the page's: `{view, load}`. Gives `{element, set(result, areas)}`. */
export function makeDid(ctx) {
  const { view } = ctx;
  const card = section('cl-did', 'What they did ', h('a', { class: 'more', href: '/reliability/changes' }, 'Open the journal'));
  const logCommand = plainCommand(view, LOG_COMMAND);

  const lineOf = (line) => h('div', { class: 'tl-item' },
    h('time', {}, line.at),
    h('span', { class: 'tl-rail' }, h('i', { class: line.kind })),
    h('div', { class: 'tl-body' },
      h('b', {}, line.title),
      h('span', { class: 'muted small' }, line.detail,
        line.task !== null && [' · ', h('button', { class: 'btn link small', type: 'button', onclick: () => view.openTask(line.task) }, 'Open the task')])));

  return {
    element: card.element,
    /** The entries in `result`, a read of the page's, with the areas' titles from `areas`. */
    set(result, areas) {
      if (result.state !== 'ok') {
        fill(card.body, unread(result, 'What the clients did', ctx.load));
        return;
      }
      const entries = Array.isArray(result.value?.entries) ? result.value.entries : [];
      const lines = historyLines(entries.slice(0, SHOWN), areas, view.time, view.session.name);
      fill(card.body,
        lines.length === 0 ? h('p', { class: 'muted' }, NONE) : h('div', { class: 'tl' }, lines.map(lineOf)),
        h('span', { class: 'small muted' }, LOGGED),
        logCommand);
    },
  };
}

/** The What no client can do card. */
export function neverCard() {
  const card = section('cl-never', 'What no client can do');
  fill(card.body, h('ul', { class: 'chk-list' },
    NEVER.map(([rule, why]) => h('li', {}, icon('lock'), h('span', {}, h('b', {}, rule), ` ${why}`)))));
  return card.element;
}
