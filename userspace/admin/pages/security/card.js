// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The cards of the Security page (design 8.9): the frame each has, the grey
// bars that stand where words are read, and the read of what only an
// administrator may see. An administrator whose console is unlocked reads it
// when the card opens, when asked again, while the card is shown and when the
// console is unlocked; one whose console is locked is asked to unlock; a
// reader is told who sees it.

import { fill, h } from '../../dom.js';
import { askToUnlock, isUnlocked } from '../../unlock.js';

/** Grey bars where words will be: still, and read by nothing. */
export const bars = () => h('div', { class: 'sec-bars', 'aria-hidden': 'true' }, h('i', { class: 'sec-bar wide' }), h('i', { class: 'sec-bar' }), h('i', { class: 'sec-bar' }));

/**
 * The card `id` named `title`, busy until it is filled: `{element, more, top, errors, body, ready}`. `more` is where
 * its buttons go beside the title, `top` where a trial's banner goes under the
 * title, `errors` where a problem goes and `body` what the card says;
 * `ready()` takes the busy mark and the bars away.
 */
export function makeCard(id, title) {
  const more = h('span', { class: 'more' });
  const heading = h('h2', { id: `${id}-h` }, title, more);
  const top = h('div', { class: 'sec-top' });
  const errors = h('div', { class: 'sec-errors' });
  const body = h('div', { class: 'sec-body' }, bars());
  const element = h('section', { class: 'card', id, 'aria-labelledby': heading.id, 'aria-busy': 'true' }, heading, top, errors, body);
  return {
    element, more, top, errors, body, ready: () => element.removeAttribute('aria-busy'),
  };
}

/**
 * Makes `card` read what an administrator reads. `action` and `args` are the
 * read; `words` is `{unlock, reader, locked, failed, missing}`: what the unlock
 * dialog says, the lines for a reader, a locked administrator and a read that
 * failed, and what a server without the command says; `draw(answer)` gives the
 * body's nodes; `every` is how often it is read again while the tab is shown.
 * Gives `{load, read, answer}`.
 */
export function adminReader(view, card, {
  action, args = {}, words, draw, every = null,
}) {
  let mode = null;
  let answer = null;
  let latest = 0;
  const again = h('button', { class: 'btn small', type: 'button', hidden: true }, 'Read again');
  card.more.append(again);
  const modeOf = () => (view.session.admin !== true ? 'reader' : isUnlocked() ? 'open' : 'locked');

  function place() {
    again.hidden = mode !== 'open';
    if (mode === 'open') return;
    fill(card.errors);
    answer = null;
    const unlock = h('button', { class: 'btn small', type: 'button' }, 'Unlock');
    unlock.addEventListener('click', async () => {
      if (await askToUnlock(view.session, words.unlock)) recheck();
    });
    fill(card.body, h('p', { class: 'muted' }, mode === 'reader' ? words.reader : words.locked), mode === 'locked' && h('div', { class: 'btns' }, unlock));
    card.ready();
  }

  /** Reads as an administrator. Only the last read counts. */
  async function read() {
    latest += 1;
    const mine = latest;
    fill(card.errors);
    const result = await view.api.run(action, args, { words: { unlock: words.unlock }, where: { slot: card.errors } })
      .catch((error) => {
        view.problem(error, { slot: card.errors });
        return { ok: false };
      });
    if (mine !== latest || view.signal.aborted) return;
    if (!result.ok) {
      // A server without the command says so in words of its own.
      if (result.error?.id === 'ADM-129') {
        fill(card.errors);
        fill(card.body, h('p', { class: 'muted' }, words.missing));
      } else if (answer === null) {
        fill(card.body, h('p', { class: 'muted' }, words.failed));
      }
      card.ready();
      return;
    }
    answer = result.result;
    fill(card.body, draw(answer));
    card.ready();
  }

  function recheck() {
    const next = modeOf();
    if (next === mode) return;
    mode = next;
    place();
    if (mode === 'open') read();
  }

  again.addEventListener('click', read);
  view.on('unlock', recheck);
  if (every !== null) view.every(every, () => {
    if (mode === 'open') read();
  });
  return {
    read,
    answer: () => answer,
    load() {
      mode = modeOf();
      place();
      return mode === 'open' ? read() : Promise.resolve();
    },
  };
}

/** Whether changes may be made now: true when the console is unlocked, else after the person unlocks it. `words` finish "You are about to". */
export const unlocked = async (view, words) => isUnlocked() || askToUnlock(view.session, words);
