// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// "What happened": the card for a start that failed or an update that was
// taken back, with what failed, what changed since the last good start, what
// the machine did, and the search for the change that broke it: start it, read
// its tries, stop it, make sure of its answer. Read when the last start
// failed.

import { fill, h } from '../../dom.js';
import { keep } from '../../lib/store.js';
import { button, makeCard, paragraphs } from './cards.js';
import {
  beforeWords, canStart, errorHref, FELL_BACK_WORDS, fellBack, happenedHeading, happenedOf, leverWords, mostTries, REPORT_HREF, searchOf,
} from './happened-model.js';

/** How often the search is read while a try waits and the page is open. */
const SEARCH_MS = 5000;

/** Draws the card into `host` (an empty place that the page keeps). */
export async function drawHappened(ctx, host) {
  const { view, data, admin } = ctx;
  const failed = data.failed.failed;
  const [notices, status] = await Promise.all([
    view.api.get('/api/1/console/notices').catch(() => null),
    view.api.get('/api/1/updates/bisect').catch(() => null),
  ]);
  if (view.signal.aborted) return;
  const lines = (Array.isArray(notices?.notices) ? notices.notices : []).find((one) => one.name === 'failed-start')?.lines;
  const info = happenedOf({ ...failed, since_good: data.failed.since_good }, { notice: lines, slots: data.slots, time: view.time });
  const card = makeCard('happened', happenedHeading(failed, view.time));
  const search = h('div', { class: 'upd-search' });
  let now = status;

  /** Runs one of the search's actions, and says what restarts when it does. */
  async function act(action, one, words, restarts) {
    const result = await ctx.run(card, action, {}, one, words);
    if (!result.ok) return;
    if (restarts) {
      const line = await ctx.pending();
      line.expect({ kind: 'bisect', try: restarts.try, of: restarts.of });
      await line.read();
    }
    await readSearch();
  }

  function drawSearch() {
    const found = searchOf(now, Date.now());
    const levers = Array.isArray(now?.levers) ? now.levers : [];
    const cannot = Array.isArray(now?.cannot) ? now.cannot : [];
    const start = canStart(now, failed);
    if (found.kind === 'idle' && !start && !fellBack(failed)) {
      fill(search);
      return;
    }
    fill(search,
      h('h3', {}, 'Find which change broke it'),
      found.kind === 'idle' && fellBack(failed) && h('p', { class: 'muted' }, FELL_BACK_WORDS),
      found.kind === 'idle' && start && [
        h('p', {}, beforeWords(levers)),
        h('ul', {}, ...levers.map((one) => h('li', {}, leverWords(one)))),
        cannot.length > 0 && [h('p', { class: 'muted' }, 'It cannot try these apart:'), h('ul', {}, ...cannot.map((one) => h('li', {}, `${one.what}: ${one.why}`)))],
        admin && h('div', { class: 'btns' }, button('Start the search', (one) => act('updates.bisect', one, { unlock: 'start the search', done: 'The search starts in a minute.' }, { try: 1, of: mostTries(levers.length) }), 'default')),
      ],
      paragraphs(found.lines),
      found.tries.length > 0 && h('ol', { class: 'upd-tries' }, ...found.tries.map((one) => h('li', {}, `${one.words}: `, one.verdict))),
      found.kind === 'running' && admin && h('div', { class: 'btns' }, button('Stop the search', (one) => act('updates.bisect-stop', one, { unlock: 'stop the search', done: 'The search is stopped.' }))),
      found.result && [
        paragraphs(found.result.lines, true),
        found.result.undo.length > 0 && [h('p', { class: 'muted' }, 'To undo it:'), ...found.result.undo.map((line) => h('pre', { class: 'cmdtext', tabindex: '0' }, line))],
        h('p', { class: 'muted' }, found.result.left),
        found.result.makeSure && admin && h('div', { class: 'btns' }, button('Make sure', (one) => act('updates.bisect-confirm', one, { unlock: 'make sure of the change', done: 'The machine restarts in a minute.' }, { try: (now.tries?.length ?? 0) + 1, of: now.of }))),
      ]);
  }

  async function readSearch() {
    now = await view.api.get('/api/1/updates/bisect').catch(() => now);
    if (!view.signal.aborted) drawSearch();
  }

  card.head(null);
  fill(card.body,
    h('p', {}, info.when),
    info.check && h('p', {}, 'Its check: ', h('a', { href: errorHref(info.check) }, info.check)),
    info.failure.length > 0 && [h('h3', {}, 'What failed'), paragraphs(info.failure)],
    info.since.length > 0 && [h('h3', {}, 'What changed since the last good start'), h('ul', {}, ...info.since.map((one) => h('li', {}, one)))],
    info.did && h('p', {}, info.did),
    h('div', { class: 'btns' }, h('a', { class: 'btn', href: REPORT_HREF }, 'Report it')),
    search);
  drawSearch();
  fill(host, card.element);
  card.element.classList.add('upd-wide');
  keep('updates.happened.seen', failed.start);
  // One timer for the page, whatever number of times the card is drawn.
  ctx.again = () => { if (now?.running) readSearch(); };
  ctx.polling ??= view.every(SEARCH_MS, () => ctx.again());
}
