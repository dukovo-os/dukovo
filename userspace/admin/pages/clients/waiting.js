// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Waiting for a yes: a card for each change that a client asked for, with
// who asked, the reason in the client's own words, the command a yes runs,
// whether it would change anything, and Approve and Refuse. A yes needs the
// console unlocked, and the change then runs as the administrator who gave
// it; a no needs only an administrator.

import { busy, fill, h } from '../../dom.js';
import { askToUnlock } from '../../unlock.js';
import { cardWords, changesWords, waitingWords } from './model.js';
import { commandBox, section, unread } from './parts.js';

const NONE = 'Nothing waits. A change that a client asks for is shown here, with the command a yes would run.';
const JUDGE = 'Judge the command, not the reason: the reason is the AI\'s. A yes runs it once, now, and the journal keeps your name and the client\'s.';
const NO_COMMAND = 'The console could not work out what this would run, so it cannot be approved. Refuse it, and have the client ask again.';

/** How often the cards say again how long each request has waited. */
const TICK_MS = 60_000;

/** The Waiting for a yes card. `ctx` is the page's: `{view, load}`. Gives `{element, set(result)}`. */
export function makeWaiting(ctx) {
  const { view } = ctx;
  const count = h('span', { class: 'pill info', hidden: true });
  const card = section('cl-waiting', 'Waiting for a yes ', count);
  // What the console said each change would do, by the request's number: the cards are drawn again and it is asked once.
  const checks = new Map();
  let pills = [];

  function changes(request) {
    const said = h('dd', {}, 'Finding out…');
    if (!checks.has(request.request)) {
      const check = view.api.post('/api/1/check', { action: request.action, args: request.args });
      checks.set(request.request, check.then((answer) => answer?.changed ?? null, () => undefined));
    }
    checks.get(request.request).then((changed) => {
      said.textContent = changesWords(changed);
    });
    return said;
  }

  /** Says yes or no to `request`, shows what came of it and reads the page again. */
  async function answer(request, words, verb, buttons) {
    fill(card.errors);
    if (verb === 'approve' && !(await askToUnlock(view.session, `approve request ${words.id}`))) return;
    await busy(buttons, async () => {
      try {
        const done = await view.api.post(`/api/1/clients/requests/${words.id}/${verb}`, {});
        if (verb === 'refuse') {
          view.notice(`Request ${words.id} is refused. Nothing was changed.`, { kind: 'ok' });
        } else if (typeof done?.task === 'number') {
          view.notice(`${words.title}: running as task ${done.task}`, { action: { label: 'Open Tasks', run: () => view.openTask(done.task) } });
        } else {
          view.notice(done?.changed === false ? `${words.title}: nothing changed.` : `${words.title}: done.`, { kind: 'ok' });
        }
      } catch (error) {
        view.problem(error, { slot: card.errors });
      }
      checks.delete(request.request);
      await ctx.load();
    });
  }

  function cardOf(request) {
    const words = cardWords(request, view.time);
    const approve = h('button', { class: 'btn default', type: 'button', disabled: words.command === null }, 'Approve');
    const refuse = h('button', { class: 'btn', type: 'button' }, 'Refuse');
    approve.addEventListener('click', () => answer(request, words, 'approve', [approve, refuse]));
    refuse.addEventListener('click', () => answer(request, words, 'refuse', [approve, refuse]));
    const pill = h('span', { class: 'pill warn' }, words.waiting);
    pills.push({ pill, asked: request.asked });
    return h('div', { class: 'ask', role: 'group', 'aria-label': `Request ${words.id}: ${words.title}` },
      h('div', { class: 'ask-head' }, h('b', {}, words.title), pill, h('span', { class: 'small muted ask-who' }, words.asked)),
      h('div', { class: 'said' }, h('span', { class: 'small muted' }, 'The client\'s reason, in its own words'), h('span', {}, words.reason)),
      words.command === null
        ? h('p', { class: 'msg', role: 'alert' }, NO_COMMAND)
        : commandBox('A yes runs this, as you', words.command, 'The command this runs').element,
      h('dl', { class: 'rows' }, h('dt', {}, 'Changes anything'), changes(request), h('dt', {}, 'Files it writes'), h('dd', {}, words.files)),
      h('div', { class: 'btns' }, approve, refuse, h('span', { class: 'small muted' }, JUDGE)));
  }

  view.every(TICK_MS, () => {
    for (const { pill, asked } of pills) pill.textContent = waitingWords(asked);
  });

  return {
    element: card.element,
    /** The requests in `result`, a read of the page's. */
    set(result) {
      pills = [];
      if (result.state !== 'ok') {
        count.hidden = true;
        fill(card.body, unread(result, 'What waits', ctx.load));
        return;
      }
      const requests = Array.isArray(result.value?.requests) ? result.value.requests : [];
      for (const id of [...checks.keys()]) {
        if (!requests.some((one) => one.request === id)) checks.delete(id);
      }
      count.hidden = requests.length === 0;
      count.textContent = String(requests.length);
      fill(card.body, requests.length === 0 ? h('p', { class: 'muted' }, NONE) : requests.map(cardOf));
    },
  };
}
