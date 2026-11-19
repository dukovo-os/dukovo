// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Overview (design 10): the Main board at `/`, from the notices at the
// top to what uses the most memory. Each card reads for itself and says in
// its own place when it cannot, so that one that fails does not take the
// others with it. The page starts them, and sets how often each reads
// again, which is in refresh.js and the table of design 10.

import { h } from '../../dom.js';
import { makeChanged } from './changed-card.js';
import { makeHealth } from './health-card.js';
import { upWords } from './model.js';
import { makeNotices } from './notices-card.js';
import { atMostEvery } from './refresh.js';
import { makeServer } from './server-card.js';
import { makeSetup } from './setup-card.js';
import { makeTop } from './top-card.js';
import { makeUsage } from './usage-card.js';

const MINUTE = 60_000;

/** How long what the machine says of itself is kept before it is read again. */
const ABOUT_MS = 290_000;

/** The least time between two reads that an event asks for. */
const EVENT_GAP_MS = 5000;

/** `console.about`, read once for every card that needs it and again after five minutes. */
function sharedAbout(view) {
  let kept = null;
  let at = 0;
  return () => {
    if (kept === null || Date.now() - at > ABOUT_MS) {
      at = Date.now();
      kept = view.api.get('/api/1/console/about').catch((error) => {
        kept = null;
        throw error;
      });
    }
    return kept;
  };
}

export async function show(view) {
  view.head({ title: 'Overview', commands: true });

  const about = sharedAbout(view);
  const notices = makeNotices(view);
  const health = makeHealth(view);
  const setup = makeSetup(view);
  const usage = makeUsage(view, { about });
  const server = makeServer(view, { about });
  const top = makeTop(view);
  // What changed alters the health list (a hand edit that is kept is no longer one) and the other way.
  // Keeping an edit sends no `change` event, so the parts' answer for the
  // list, kept for 5 seconds, is forgotten before Health reads it again.
  const changed = makeChanged(view, {
    about,
    reload() {
      view.forgetParts();
      return Promise.all([changed.load(), health.load()]);
    },
  });
  const upLine = h('p', { class: 'small muted ov-up' });

  view.main.append(h('div', { class: 'ov' },
    notices.element,
    h('div', { class: 'grid ov-cards' }, health.element, setup.element, changed.element),
    usage.element,
    h('div', { class: 'grid two ov-more' }, server.element, top.element),
    upLine));

  const everything = [notices, health, setup, changed, usage, server, top];
  for (const card of everything) card.load();
  about().then((facts) => { upLine.textContent = upWords(facts) ?? ''; }, () => {});

  // A task or a change in the journal makes a card read again, at most every 5 seconds.
  const healthSoon = atMostEvery(health.load, EVENT_GAP_MS);
  const changedSoon = atMostEvery(changed.load, EVENT_GAP_MS);
  view.signal.addEventListener('abort', () => {
    healthSoon.stop();
    changedSoon.stop();
  }, { once: true });
  view.on('task', healthSoon);
  view.on('change', () => {
    changedSoon();
    healthSoon();
  });
  // The server answers again after being away: everything is read at once.
  view.on('online', () => everything.forEach((card) => card.load()));

  view.every(MINUTE, () => {
    health.load();
    top.load();
    if (usage.due()) usage.load();
  });
  view.every(5 * MINUTE, () => {
    notices.load();
    server.load();
    changed.load();
  });
}
