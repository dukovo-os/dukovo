// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The recent log of one service (design 4.4): its last 20 lines, in the list
// the Logs page draws (lib/log-list.js, read when the card first has lines).
// An unlocked administrator reads the whole log of the service; anyone else
// reads the lines of their own programs, with the words that say who sees
// the rest. This is the one module of the folder that uses the shared list.
// A read as an administrator is made when the card opens, when the person
// asks again and when the console is unlocked or locked, never on a timer
// (design 3.4).

import { busy, fill, h } from '../../dom.js';
import { askToUnlock, isUnlocked } from '../../unlock.js';
import { quiet } from './cards.js';

/** The lines the card reads. */
const LINES = 20;

/** The address of the Logs page for the service `name`. */
const logsHref = (name) => `/logs?${new URLSearchParams({ service: name })}`;

/** The line of words, and the button if there is one, that says who sees the rest of the log. */
const OTHERS = Object.freeze({
  reader: 'Administrators see the service\'s log.',
  locked: 'You see the lines of your own programs. Unlock to see the service\'s log.',
});

/** The card of the Recent log of the service `name`: `{element, load}`. */
export function makeRecentLog(view, name) {
  const errors = h('div', { class: 'svc-log-errors' });
  const holder = h('div', { class: 'svc-log-list', 'aria-hidden': 'true' }, ...[0, 1, 2].map(() => h('span', { class: 'svc-bar wide' })));
  const others = h('div', { class: 'svc-log-others' });
  const said = h('span', { class: 'sr-only', role: 'status' });
  const again = h('button', { class: 'btn small', type: 'button' }, 'Read again');
  const element = h('section', { class: 'card', id: 'log', 'aria-labelledby': 'log-h' },
    h('h2', { id: 'log-h' }, 'Recent log'),
    errors, holder, others,
    h('div', { class: 'btns' }, again, h('a', { class: 'btn small', href: logsHref(name) }, 'Open in Logs'), said));

  const args = { service: name, lines: LINES };
  const modeOf = () => (view.session.admin && isUnlocked() ? 'all' : 'own');
  let mode = modeOf();
  let latest = 0;
  let list = null;

  /** The list of lines, made when the first lines come. */
  async function listOf() {
    if (list === null) {
      const { makeLogList } = await import('../../lib/log-list.js');
      list = makeLogList({ time: view.time });
    }
    return list;
  }

  /** The words and the button under the list: who sees the rest of the log. */
  function drawOthers() {
    if (mode === 'all') return fill(others);
    const unlock = h('button', { class: 'btn small', type: 'button' }, 'Unlock');
    unlock.addEventListener('click', async () => {
      if (await askToUnlock(view.session, `read the log of ${name}`)) recheck();
    });
    return fill(others, h('p', { class: 'muted' }, view.session.admin ? OTHERS.locked : OTHERS.reader), view.session.admin && h('div', { class: 'btns' }, unlock));
  }

  /** Reads the lines and draws them. Only the last read counts. */
  async function load() {
    latest += 1;
    const mine = latest;
    mode = modeOf();
    errors.replaceChildren();
    drawOthers();
    const result = await busy([again], () => view.api.run(mode === 'all' ? 'logs.read-all' : 'logs.read', args, {
      words: { unlock: `read the log of ${name}` }, where: { slot: errors },
    }).catch((error) => {
      view.problem(error, { slot: errors });
      return { ok: false };
    }));
    if (mine !== latest) return;
    holder.removeAttribute('aria-hidden');
    if (!result.ok) {
      fill(holder, quiet('The log could not be read.'));
      return;
    }
    const records = Array.isArray(result.result) ? result.result : [];
    if (records.length === 0) {
      fill(holder, quiet(mode === 'all' ? 'The service has written no lines yet.' : 'You have no lines of your own from this service.'));
      said.textContent = 'No lines.';
      return;
    }
    try {
      const shown = await listOf();
      if (mine !== latest) return;
      shown.set(records);
      fill(holder, shown.element);
      said.textContent = `${records.length} ${records.length === 1 ? 'line' : 'lines'}.`;
    } catch {
      fill(holder, quiet('The list of lines could not be shown.'));
    }
  }

  /** Reads again when the console was unlocked or locked since the last read. */
  function recheck() {
    if (modeOf() !== mode) load();
  }

  again.addEventListener('click', load);
  view.on('unlock', recheck);
  return { element, load };
}
