// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Who may reach the console (design 8.7 and 9.2): the row, and the two changes
// of it, each tried as the trial `console`. Making the console public asks for
// care and offers the second step first; making it private, from a browser that
// is outside the networks it would then answer, is said to cut that browser off,
// and is only tried. The browser's own reach over the new settings is asked by
// C4's banner, through `GET /api/1/console/fresh/console`.

import { fill, h } from '../../dom.js';
import { makeTrialBanner } from '../../lib/trial-banner.js';
import { atMostEvery } from '../overview/refresh.js';
import { makeCard, unlocked } from './card.js';
import { isPublic, reachRow } from './rules-model.js';

const EVENT_GAP_MS = 2000;

/**
 * The card. `reread()` reads the rules and the reach again after a change.
 * Gives `{element, set(answer), load}`: `set` draws the reach of `admin
 * settings --json`'s answer.
 */
export function makeReach(view, { reread }) {
  const card = makeCard('reach', 'Who may reach the console');
  const change = h('button', { class: 'btn small', type: 'button', hidden: true, 'data-action': 'reach-change' });
  card.more.append(change);
  const banner = makeTrialBanner(view, {
    id: 'console',
    what: () => 'Who may reach the console is being tried.',
    keep: 'console.keep',
    undo: 'console.undo',
    onState: ({ running, ended }) => {
      tried = running;
      draw();
      if (ended) soon();
    },
  });
  card.top.append(banner.element);
  let answer = null;
  let tried = false;

  const running = () => tried || banner.running();

  function draw() {
    if (answer === null) return;
    const admin = view.session.admin === true;
    const open = isPublic(answer.console);
    fill(card.body, h('dl', { class: 'rows' }, h('dt', {}, 'Console'), h('dd', {}, reachRow(answer.console))));
    change.hidden = !admin;
    change.textContent = open ? 'Only this network…' : 'Make the console public…';
    change.setAttribute('aria-disabled', String(running()));
    card.ready();
  }

  /** The form that changes the reach to `reach`, `any` or `private`. */
  async function openChange(reach) {
    if (!(await unlocked(view, reach === 'any' ? 'make the console public' : 'limit the console to this network'))) return;
    const { openReachForm } = await import('./reach-form.js');
    await openReachForm(view, {
      reach, answer, banner, reread,
    });
  }

  change.addEventListener('click', () => {
    if (change.getAttribute('aria-disabled') !== 'true' && answer !== null) openChange(isPublic(answer.console) ? 'private' : 'any');
  });
  const soon = atMostEvery(() => reread(), EVENT_GAP_MS);
  view.signal.addEventListener('abort', soon.stop, { once: true });
  return {
    element: card.element,
    load: () => banner.read(),
    set(next) {
      answer = next ?? {};
      draw();
    },
    fail(error) {
      view.problem(error, { slot: card.errors });
      fill(card.body, h('p', { class: 'muted' }, 'The console\'s settings could not be read.'));
      card.ready();
    },
  };
}
