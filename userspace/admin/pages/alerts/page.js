// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Alerts: what is active and what happened lately, where alerts go (email,
// webhooks, browsers) with what each is told of and how it last did, a test
// alert, and the steps to get alerts in this browser and to set up a phone.
// Every read is an administrator's and is made when the page opens, when the
// person asks again, after a change and when the console is locked or
// unlocked, never on a timer.

import { fill, h, icon } from '../../dom.js';
import { recall } from '../../lib/store.js';
import { makeActive, makeRecent } from './active.js';
import { makeChannels } from './channels.js';
import { listOf } from './channels-model.js';
import { makeEmail } from './email.js';
import { alertdDown, DOWN_WORDS, START_COMMAND } from './model.js';
import { makePush } from './push.js';
import { makeReads } from './reads.js';
import { PHONE_DONE_KEY, wantsCard, wantsPanel } from './phone-flag.js';
import { deliveriesOf, NO_CHANNEL_WORDS, testRows } from './test-model.js';

const LEAD = 'One message per change, with this server\'s name. An alert is told when it starts, when it gets worse and when it clears, never again while it stays.';

/** The least width, in pixels, of a screen that is not a phone's. */
const PHONE_WIDTH = 720;

/** What a reader sees: the page is for administrators, since every alerts command needs root. */
export async function reader(view) {
  view.head({ title: 'Alerts' });
  view.main.append(h('div', { class: 'banner warn' }, icon('lock'), h('div', {},
    h('p', {}, h('b', {}, 'Alerts are for administrators.'), ` ${view.session.name}'s account can look but not change. Ask an administrator, or ask to be made one.`),
    h('div', { class: 'btns' }, h('a', { class: 'btn small', href: '/accounts' }, 'Who are the administrators?')))));
}

export async function show(view) {
  const reads = makeReads(view);
  const test = h('button', { class: 'btn default', type: 'button' }, 'Send a test alert');
  view.head({
    title: 'Alerts', commands: true, buttons: [test], lead: LEAD,
  });
  const ctx = {
    view,
    reads,
    load: () => load(),
    test: (button) => sendTest(button),
    pushId: () => push.id(),
    removed: (channel) => push.removed(channel),
    hostname: undefined,
    install: null,
    installed: null,
    push: null,
  };
  const active = makeActive(ctx);
  const recent = makeRecent(ctx);
  const channels = makeChannels(ctx);
  const email = makeEmail(ctx);
  const push = makePush(ctx);
  ctx.push = push;

  const gate = h('div', {});
  const down = h('div', {});
  const panel = h('div', {});
  const cards = h('div', { class: 'al-grid' }, active.element, push.element, channels.element, email.element, recent.element);
  view.main.append(h('div', { class: 'al' }, gate, down, panel, cards));
  test.addEventListener('click', () => sendTest(test));

  // The phone's panel, when the code that this page makes opened it; else, on a computer, the card of steps.
  const admin = view.session.admin === true;
  if (wantsPanel({ query: view.query, done: recall(PHONE_DONE_KEY, false), admin })) {
    globalThis.addEventListener?.('beforeinstallprompt', (event) => {
      event.preventDefault();
      ctx.install = event;
      ctx.installed?.();
    }, { signal: view.signal });
    import('./phone-setup.js').then(({ makePhonePanel }) => panel.replaceChildren(makePhonePanel(ctx).element));
  } else if (wantsCard({ narrow: globalThis.innerWidth <= PHONE_WIDTH, admin })) {
    import('./phone-setup.js').then(({ makePhoneCard }) => {
      const phone = makePhoneCard(ctx);
      cards.append(phone.element);
      return phone.draw();
    });
  }

  let latest = 0;
  async function load() {
    latest += 1;
    const mine = latest;
    const [list, history, status] = await Promise.all(['/api/1/alerts', '/api/1/alerts/history', '/api/1/alerts/channels'].map((path) => reads.read(path)));
    if (mine !== latest || view.signal.aborted) return;
    const blocked = [list, history, status].find((one) => one.state === 'locked' || one.state === 'reader');
    cards.hidden = blocked !== undefined;
    fill(gate, blocked && reads.note(blocked, 'the alerts', load));
    const gone = [list, status].some((one) => one.state === 'error' && alertdDown(one.error));
    fill(down, gone && h('div', { class: 'banner warn' }, icon('warn'), h('div', {},
      h('p', {}, DOWN_WORDS, h('span', { class: 'mono' }, START_COMMAND)),
      h('div', { class: 'btns' }, h('a', { class: 'btn small', href: '/services/alertd' }, 'alertd\'s service')))));
    if (blocked) return;
    active.set(list);
    recent.set(history);
    channels.set(status);
    email.set(status.state === 'ok' ? channels.rows() : []);
    if (status.state === 'ok') push.set(listOf(status.value));
  }

  /** Sends a test alert to every channel, and shows what each said. */
  async function sendTest(button) {
    const result = await view.api.run('alerts.test', {}, {
      button,
      words: {
        unlock: 'send a test alert', running: 'Sending a test alert', done: 'The test alert was sent.', busy: 'Sending…',
      },
    });
    if (!result.ok) return;
    const deliveries = deliveriesOf(result.result);
    channels.showTest(deliveries === null ? 'The test alert was sent. This server did not say where it reached.' : deliveries.length === 0 ? NO_CHANNEL_WORDS : testRows(deliveries));
    await load();
  }

  view.on('unlock', load);
  await load();
}
