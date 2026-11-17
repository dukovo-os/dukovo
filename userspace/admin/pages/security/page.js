// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Security (design 8): one route, /security. The score and the checks with each
// finding's button, the sign-in rules, the addresses blocked after wrong
// sign-ins, SSH, who may reach the console and private access, each a card
// that scrolls into view by its id (#checks, #alerts, #sign-in, #blocked, #ssh,
// #reach, #private). SSH's settings and the console's reach are changed as trials: the
// banner of each is at the top of its card. The sign-in rules and the console's
// reach come from the same read, `admin settings --json`.

import { h } from '../../dom.js';
import { PAGES } from '../../pages.js';
import { atMostEvery } from '../overview/refresh.js';
import { makeAlerts } from './alerts-card.js';
import { makeBlocked } from './blocked.js';
import { makeFindings } from './findings.js';
import { makePrivate } from './private.js';
import { makeReach } from './reach.js';
import { makeSignIn } from './sign-in.js';
import { makeSsh } from './ssh.js';

/** The least time between two reads of the rules that an event asks for. */
const EVENT_GAP_MS = 2000;

/** Whether the console has the page `id`. */
const hasPage = (id) => PAGES.some((entry) => entry.id === id && entry.load !== null);

export async function show(view) {
  view.head({ title: 'Security', commands: true });
  let latest = 0;

  const ssh = makeSsh(view);
  const findings = makeFindings(view, { hasPage, openSsh: (preset) => ssh.open(preset) });
  const signIn = makeSignIn(view, { reread: () => readRules() });
  const reach = makeReach(view, { reread: () => readRules() });
  const alerts = makeAlerts(view);
  const blocked = makeBlocked(view);
  const priv = makePrivate(view);
  view.main.append(h('div', { class: 'sec' },
    findings.element,
    alerts.element,
    h('div', { class: 'grid two' }, signIn.element, blocked.element),
    h('div', { class: 'grid two' }, ssh.element, reach.element),
    priv.element));

  /** Reads `admin settings --json`, which holds the sign-in rules and the console's reach. Only the last read counts. */
  async function readRules() {
    latest += 1;
    const mine = latest;
    try {
      const found = await view.api.get('/api/1/security/sign-in');
      if (mine !== latest) return;
      signIn.set(found);
      reach.set(found);
    } catch (error) {
      if (mine !== latest || error?.name === 'AbortError') return;
      signIn.fail(error);
      reach.fail(error);
    }
  }

  const soon = atMostEvery(readRules, EVENT_GAP_MS);
  view.signal.addEventListener('abort', soon.stop, { once: true });
  view.on('change', soon);
  await Promise.all([findings.load(), readRules(), ssh.load(), reach.load(), alerts.load(), blocked.load(), priv.load()]);
}
