// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A card of the Firewall page that shows what only an administrator may read
// (design 3.4 and 10.3): what was blocked and what listens. An administrator
// whose console is unlocked reads it when the page opens, when they ask again
// and when the console is unlocked, never on a timer; one whose console is
// locked is asked to unlock; a reader is told who sees it.

import { busy, fill, h } from '../../dom.js';
import { askToUnlock, isUnlocked } from '../../unlock.js';

/**
 * The card `id` with `title`. `action` and `args` are the read, which runs as
 * an administrator; `words` is `{unlock, reader, locked, open}`: what the
 * unlock dialog says, and the lines for a reader and a locked administrator;
 * `draw(result)` makes the card's body from the answer. Gives `{element,
 * load, set}`: `load()` reads when the console is unlocked, `set(extra)`
 * hands `draw` what else it needs, and draws again.
 */
export function makeAdminCard(view, {
  id, title, action, args, words, draw,
}) {
  const errors = h('div', { class: 'fw-errors' });
  const body = h('div', { class: 'fw-body' });
  const again = h('button', { class: 'btn small', type: 'button', hidden: true }, 'Read again');
  const head = h('h2', { id: `${id}-h` }, title, h('span', { class: 'more' }, again));
  const element = h('section', { class: 'card', id, 'aria-labelledby': head.id }, head, errors, body);
  let mode = null;
  let answer = null;
  let extra = {};
  let latest = 0;

  const modeOf = () => (view.session.admin !== true ? 'reader' : isUnlocked() ? 'open' : 'locked');

  /** The words and the button that stand where the answer would be. */
  function drawPlace() {
    again.hidden = mode !== 'open';
    if (mode === 'open') return;
    errors.replaceChildren();
    answer = null;
    const unlock = h('button', { class: 'btn small', type: 'button' }, 'Unlock');
    unlock.addEventListener('click', async () => {
      if (await askToUnlock(view.session, words.unlock)) recheck();
    });
    fill(body, h('p', { class: 'muted' }, mode === 'reader' ? words.reader : words.locked), mode === 'locked' && h('div', { class: 'btns' }, unlock));
  }

  /** Draws the answer, with what `set` was given. */
  function drawAnswer() {
    if (mode === 'open' && answer !== null) fill(body, draw(answer, extra));
  }

  /** Reads as an administrator. Only the last read counts. */
  async function read() {
    latest += 1;
    const mine = latest;
    errors.replaceChildren();
    const result = await busy([again], () => view.api.run(action, args, { words: { unlock: words.unlock }, where: { slot: errors } })
      .catch((error) => {
        view.problem(error, { slot: errors });
        return { ok: false };
      }));
    if (mine !== latest) return;
    if (!result.ok) {
      if (answer === null) fill(body, h('p', { class: 'muted' }, words.failed));
      return;
    }
    answer = result.result;
    drawAnswer();
  }

  /** Reads again when the console was unlocked or locked since the last time. */
  function recheck() {
    const next = modeOf();
    if (next === mode) return;
    mode = next;
    drawPlace();
    if (mode === 'open') read();
  }

  again.addEventListener('click', read);
  view.on('unlock', recheck);
  return {
    element,
    load() {
      mode = modeOf();
      drawPlace();
      if (mode === 'open') return read();
      return Promise.resolve();
    },
    set(next) {
      extra = next;
      drawAnswer();
    },
  };
}
