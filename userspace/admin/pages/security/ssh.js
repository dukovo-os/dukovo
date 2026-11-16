// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// SSH (design 8.6 and 9.1): the row, the switch, the form that changes who may
// sign in and how, and the banner of the trial `ssh` with what to check while
// it runs. A change that takes a way in from someone is only tried: it is
// undone unless it is kept. The browser does not come over SSH, so nothing
// proves that a new connection works; the page asks the person to open one and
// lists the SSH sign-ins since the change. Keep is the person's choice.

import { fill, h, icon } from '../../dom.js';
import { makeTrialBanner } from '../../lib/trial-banner.js';
import { isUnlocked } from '../../unlock.js';
import { atMostEvery } from '../overview/refresh.js';
import { makeCard, unlocked } from './card.js';
import {
  beganAt, checkWords, connectCommand, hasMistakes, keepNote, mistakeWords, rowWords, signInsSince, signInWords,
} from './ssh-model.js';

/** How often the settings are read while a try runs, and the least time between two reads that an event asks for. */
const TRY_READ_MS = 5000;
const EVENT_GAP_MS = 2000;

/** The journal's newest rows that are read to find when a try began. */
const JOURNAL_ROWS = 100;

/**
 * Spark's trial `ssh` has no `fresh`: the browser does not come over SSH. The
 * banner asks the console whether this browser reaches it over the new
 * settings, and for SSH the answer is given here, so that Keep is the person's
 * to press.
 */
function withoutFresh(view) {
  const get = (path) => (path === '/api/1/console/fresh/ssh' ? Promise.resolve({ fresh: true, from: null, via: null }) : view.api.get(path));
  return Object.create(view, { api: { value: { ...view.api, get } } });
}

/**
 * The card. Gives `{element, load, open(preset)}`: `open` is the form, for a
 * finding of the checks that is about SSH.
 */
export function makeSsh(view) {
  const card = makeCard('ssh', 'SSH');
  const knob = h('span', { class: 'sw', 'aria-hidden': 'true' });
  const sw = h('button', { class: 'sec-switch', type: 'button', role: 'switch', 'aria-checked': 'false', hidden: true, 'data-action': 'ssh-switch' }, knob, 'SSH');
  const change = h('button', { class: 'btn small', type: 'button', hidden: true, 'data-action': 'ssh-change' }, 'Change…');
  card.more.append(sw, change);
  const check = h('div', { class: 'sec-ssh-check', hidden: true });
  const banner = makeTrialBanner(withoutFresh(view), {
    id: 'ssh',
    what: () => 'SSH\'s settings are being tried.',
    keep: 'ssh.keep',
    undo: 'ssh.undo',
    onState: ({ running }) => {
      tried = running;
      if (running) began();
      else since = null;
      draw();
      soon();
    },
  });
  card.top.append(h('div', { class: 'sec-ssh-try' }, check, banner.element));

  let show = null;
  let tried = false;
  let since = null;
  let signIns = null;
  let latest = 0;

  const running = () => tried || banner.running() || show?.trial != null;

  /** When the try began: the journal's row for it, else now, which only misses a sign-in made before this page saw the try. */
  async function began() {
    if (since !== null) return;
    since = Date.now();
    try {
      const found = beganAt((await view.api.get(`/api/1/changes?limit=${JOURNAL_ROWS}`))?.changes);
      if (found !== null) since = Math.min(since, found);
    } catch {
      // A journal that cannot be read leaves the time this page saw the try.
    }
    pollSignIns();
  }

  /** The SSH sign-ins since the change: for an administrator whose console is unlocked, who may read when each account signed in. */
  async function pollSignIns() {
    if (!running() || since === null) return;
    if (view.session.admin === true && isUnlocked()) {
      try {
        signIns = signInsSince(await view.api.get('/api/1/sign-ins/last'), since);
      } catch (error) {
        if (error?.name === 'AbortError') return;
        signIns = null;
      }
    } else {
      signIns = null;
    }
    drawCheck();
  }

  function drawCheck() {
    check.hidden = !running();
    if (check.hidden) return;
    const note = signIns !== null ? keepNote(signIns) : (view.session.admin === true ? 'Unlock to see who signs in over SSH since the change.' : 'Administrators see who signs in over SSH since the change.');
    fill(check,
      h('p', {}, 'Open a new SSH connection from your computer now: ', h('code', {}, connectCommand(view.session.name, view.host ?? location.hostname)), '. ', checkWords),
      signIns !== null && signIns.length > 0 && h('ul', { class: 'sec-signins' }, ...signIns.map((one) => h('li', {}, signInWords(one, view.time)))),
      h('p', { class: 'small muted', role: 'status' }, note));
  }

  function draw() {
    if (show === null) return;
    card.ready();
    const admin = view.session.admin === true;
    const busy = running();
    const mistakes = hasMistakes(show);
    sw.hidden = !admin;
    sw.setAttribute('aria-checked', String(show.on !== false));
    knob.classList.toggle('on', show.on !== false);
    sw.setAttribute('aria-disabled', String(busy));
    change.hidden = !admin;
    change.setAttribute('aria-disabled', String(busy || mistakes));
    const denied = [show.deny_users?.length > 0 && `users ${show.deny_users.join(', ')}`, show.deny_groups?.length > 0 && `groups ${show.deny_groups.join(', ')}`].filter(Boolean);
    fill(card.body,
      mistakes && h('div', { class: 'banner bad', role: 'status' }, icon('warn'), h('div', {},
        h('p', {}, h('b', {}, mistakeWords(show))),
        h('p', {}, 'SSH goes on with the settings it read last; the form cannot change the file until it is fixed. ', h('code', {}, 'ssh-server show'), ' lists each line that is wrong. Correct the file /system/settings/ssh/sshd_config as an administrator, or put back an earlier version from the What changed page.'))),
      h('dl', { class: 'rows' },
        h('dt', {}, 'SSH'), h('dd', {}, rowWords(show)),
        show.host_key && [h('dt', {}, 'Host key'), h('dd', { class: 'mono' }, show.host_key)],
        denied.length > 0 && [h('dt', {}, 'Denied'), h('dd', {}, `${denied.join(' · ')} (kept as they are; the form does not edit them)`)],
        show.forwarding && [h('dt', {}, 'Forwarding'), h('dd', {}, `${show.forwarding}, gateway ports ${show.gateway_ports ?? 'no'} (shown here; the form does not edit them)`)]));
    drawCheck();
  }

  /** Reads SSH's settings. Only the last read counts. */
  async function read() {
    latest += 1;
    const mine = latest;
    try {
      const found = await view.api.get('/api/1/ssh');
      if (mine !== latest) return;
      show = found;
      fill(card.errors);
      if (found.trial != null && !banner.running()) banner.read();
    } catch (error) {
      if (mine !== latest || error?.name === 'AbortError') return;
      view.problem(error, { slot: card.errors });
      if (show === null) {
        fill(card.body, h('p', { class: 'muted' }, 'SSH\'s settings could not be read.'));
        card.ready();
      }
      return;
    }
    draw();
  }

  /** The form, for the settings now; `preset` is `keys-only`, `who` or `off`. Its code is read when it is first asked for. */
  async function open(preset = null) {
    if (show === null || running() || hasMistakes(show) || !(await unlocked(view, 'change SSH\'s settings'))) return;
    const { openSshForm } = await import('./ssh-form.js');
    await openSshForm(view, {
      show, preset, banner, read,
    });
  }

  change.addEventListener('click', () => {
    if (change.getAttribute('aria-disabled') !== 'true') open();
  });
  sw.addEventListener('click', async () => {
    if (sw.getAttribute('aria-disabled') === 'true' || show === null) return;
    if (show.on !== false) {
      open('off');
      return;
    }
    if (hasMistakes(show) || !(await unlocked(view, 'turn SSH on'))) return;
    const { runCalls } = await import('./ssh-form.js');
    if (await runCalls(view, [{ action: 'ssh.on', args: {} }], sw, card.errors)) {
      await read();
    }
  });

  const soon = atMostEvery(read, EVENT_GAP_MS);
  view.signal.addEventListener('abort', soon.stop, { once: true });
  view.on('change', soon);
  view.every(TRY_READ_MS, () => {
    if (!running()) return;
    read();
    pollSignIns();
  });
  return { element: card.element, load: () => Promise.all([read(), banner.read()]), open };
}
