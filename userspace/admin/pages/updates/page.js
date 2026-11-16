// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Updates: when the system, its firmware set and the apps were last checked,
// what each can be updated to, and what an administrator may do about it.
// The page reads the last check, the settings, the checks, the failed start
// and the apps when it opens, and again when a change or a task of its own
// ends; the slots only for an administrator whose console is unlocked. What
// happened after a failed start, the history, the dialogs and the restart are
// other modules, read when a person asks for them.

import { fill, h, icon } from '../../dom.js';
import { askToUnlock, isUnlocked } from '../../unlock.js';
import { makeApps, makeSecurity } from './apps.js';
import { button, makeCard } from './cards.js';
import { makeChecks } from './checks.js';
import { makeHeld } from './held.js';
import {
  appOffers, checkedWords, checkOf, runningOf, slotVersion, withHolds,
} from './model.js';
import { makeSettings } from './settings.js';
import { makeFirmware, makeSystem } from './system.js';

const PROMISE = 'Each update of the system starts as a try: if it fails a required check, the machine goes back to the version before by itself. The system and the apps update apart, so one can go back without the other. This page comes back by itself after a restart.';

/** What the page reads when it opens, by name. */
const READS = {
  last: '/api/1/updates', settings: '/api/1/updates/settings', checks: '/api/1/updates/checks', failed: '/api/1/updates/failed', apps: '/api/1/apps', about: '/api/1/console/about', tasks: '/api/1/tasks',
};

/** The tasks of this page, and which card shows each. */
const WATCHED = ['updates.apply', 'apps.update', 'apps.update-all'];
const running = (task) => task.state === 'running' || task.state === 'waiting';
const cardOf = (task) => (['system', 'firmware'].includes(task.args?.item) ? task.args.item : 'apps');

export async function show(view) {
  const admin = view.session.admin === true;
  const data = {
    check: null, settings: null, checks: null, failed: null, apps: null, flaws: null, slots: null, slotsState: admin ? 'locked' : 'reader', running: null,
  };
  const tasks = new Map();
  const notes = new Map();
  let line = null;
  let latest = 0;
  let checking = false;

  const checked = h('p', { class: 'upd-checked' });
  const say = h('p', { class: 'sr-only', role: 'status' });
  const checkNow = button('Check now', (one) => checkAgain(one));
  const banner = h('div', { class: 'banner info', hidden: true });
  const happened = h('div', { class: 'upd-wide' });
  const history = makeCard('history', 'History');
  const ctx = {
    view,
    admin,
    data,
    notes,
    pendingHost: null,
    task: (kind) => [...tasks.values()].find((task) => running(task) && cardOf(task) === kind) ?? null,
    say: (words) => { say.textContent = words; },
    run: (card, action, args, one, words) => {
      fill(card.errors);
      return view.api.run(action, args, { button: one, words, where: { slot: card.errors } });
    },
    async reload(focus = null) {
      await read();
      focus?.focus();
    },
    begin(id, action, args) {
      tasks.set(id, {
        id, action, args, state: 'running', step: 0, steps: 0,
      });
      drawAll();
    },
    show(id) {
      const target = view.main.querySelector(`#${id}`);
      target?.scrollIntoView?.();
      target?.querySelector('h2')?.focus();
    },
    async pending() {
      if (line === null) {
        const { pendingRestart } = await import('../../lib/pending-restart.js');
        line = pendingRestart(view);
        (ctx.pendingHost ?? view.slot).append(line.element);
      }
      return line;
    },
    stagedVersion: () => (data.slots?.staged ? slotVersion(data.slots, data.slots.staged.slot) : null),
    async restart() {
      const { restartNow } = await import('../../lib/restart.js');
      const version = ctx.stagedVersion();
      await restartNow(view, {
        why: version ? `To finish the update to ${version}.` : '', kind: 'update', version, pending: await ctx.pending(),
      });
    },
    async restartAt(time) {
      const { restartAt } = await import('../../lib/restart.js');
      await restartAt(view, time, { kind: 'update', version: ctx.stagedVersion(), pending: await ctx.pending() });
    },
    async readSlots() {
      if (admin && !isUnlocked() && !(await askToUnlock(view.session, 'read the slots'))) return;
      await readSlotsNow();
      drawAll();
    },
  };

  const system = makeSystem(ctx);
  const apps = makeApps(ctx);
  const security = makeSecurity(ctx);
  const firmware = makeFirmware(ctx);
  const held = makeHeld(ctx);
  const settings = makeSettings(ctx);
  const checks = makeChecks(ctx);
  const showHistory = button('Show the history', async (one) => {
    const { drawHistory } = await import('./history.js');
    await drawHistory(ctx, history, one);
  });
  fill(history.body, h('div', { class: 'btns' }, showHistory));
  history.head(null);
  history.element.classList.add('upd-wide');
  for (const one of [system, apps, checks]) one.element.classList.add('upd-wide');

  // The cards come in when the first read is in, so that none says "Not checked yet" before it knows.
  let cards = [system, apps, security, firmware, held, settings, checks, history].map((one) => one.element);
  const host = h('div', { class: 'upd' },
    checked, h('p', { class: 'muted' }, PROMISE), !admin && h('p', { class: 'muted' }, 'An administrator can stage updates, hold versions and change these settings.'),
    banner, say, happened);
  checked.textContent = 'Reading the last check…';
  view.head({ title: 'Updates', commands: true, buttons: [checkNow] });
  view.main.append(host);

  function drawAll() {
    const asOf = data.check?.checkedMs ? ` As of ${view.time.clock(data.check.checkedMs / 1000)}.` : '';
    checked.textContent = checking ? `Asking the update server…${asOf}` : checkedWords(data.check, view.time);
    const staged = data.slots?.staged;
    fill(banner, staged && [icon('info'), h('div', {}, h('p', {}, h('b', {}, 'Restart to finish the update'), ` ${ctx.stagedVersion() ?? 'The update'} starts at the next restart.`),
      admin && h('div', { class: 'btns' }, button('Restart now…', () => ctx.restart(), 'default')))]);
    banner.hidden = !staged;
    for (const one of [system, apps, security, firmware, held, settings, checks]) one.draw();
  }

  async function readSlotsNow() {
    data.slots = null;
    if (!admin) {
      data.slotsState = 'reader';
    } else if (!isUnlocked()) {
      data.slotsState = 'locked';
    } else {
      try {
        data.slots = await view.api.get('/api/1/updates/slots');
        data.slotsState = 'ok';
      } catch (error) {
        data.slotsState = error?.id === 'ADM-106' ? 'locked' : error?.id === 'ADM-122' ? 'reader' : 'error';
        if (data.slotsState === 'error' && error?.name !== 'AbortError') view.problem(error, { slot: view.slot });
      }
    }
  }

  /** Reads everything again; only the last read to ask is drawn. */
  async function read() {
    latest += 1;
    const mine = latest;
    const names = Object.keys(READS);
    const found = await Promise.allSettled(names.map((name) => view.api.get(READS[name])));
    const got = Object.fromEntries(names.map((name, at) => [name, found[at].status === 'fulfilled' ? found[at].value : null]));
    const lastError = found[0].status === 'rejected' && found[0].reason?.name !== 'AbortError' ? found[0].reason : null;
    if (mine !== latest || view.signal.aborted) return;
    data.settings = got.settings;
    data.check = withHolds(checkOf(got.last), data.settings);
    data.checks = got.checks;
    data.failed = got.failed;
    data.apps = Array.isArray(got.apps?.bundles) ? got.apps.bundles : null;
    data.running = runningOf(got.about);
    tasks.clear();
    for (const task of Array.isArray(got.tasks?.tasks) ? got.tasks.tasks : []) if (WATCHED.includes(task.action) && running(task)) tasks.set(task.id, task);
    await readSlotsNow();
    data.flaws = null;
    if (data.check?.appFlaws > 0 && appOffers(data.check).length > 0) {
      data.flaws = (await view.api.get('/api/1/updates/advisories').catch(() => null))?.apps ?? null;
    }
    if (mine !== latest || view.signal.aborted) return;
    if (cards !== null) {
      host.append(...cards);
      cards = null;
    }
    drawAll();
    if (lastError) view.problem(lastError, { slot: view.slot });
    if (data.failed?.failed) {
      const { drawHappened } = await import('./happened.js');
      await drawHappened(ctx, happened);
    } else {
      fill(happened);
    }
    const power = await view.api.get('/api/1/power').catch(() => null);
    if (power?.shutdown) (await ctx.pending()).set(power.shutdown);
    else if (line) line.set(null);
  }

  async function checkAgain(one) {
    checking = true;
    drawAll();
    const result = await view.api.run('updates.check', {}, { button: one, where: { slot: view.slot }, words: { unlock: 'check for updates', busy: 'Asking the update server…', done: 'The check is done.' } });
    checking = false;
    if (result.ok && Array.isArray(result.result?.offers)) data.check = withHolds(checkOf(result.result), data.settings);
    drawAll();
    if (result.ok) ctx.say(checkedWords(data.check, view.time));
  }

  view.on('task', async (event) => {
    if (!WATCHED.includes(event.action) && !tasks.has(event.id)) return;
    if (!tasks.has(event.id)) {
      try {
        tasks.set(event.id, await view.api.get(`/api/1/tasks/${event.id}`));
      } catch {
        return;
      }
    }
    const task = { ...tasks.get(event.id), ...event };
    tasks.set(event.id, task);
    if (running(task)) {
      drawAll();
      return;
    }
    if (task.state !== 'done' && task.action === 'apps.update' && task.args?.id) notes.set(task.args.id, task.note || task.words || 'It asks for more than the version before.');
    ctx.say(task.state === 'done' ? 'The update is done.' : 'The update did not finish.');
    await read();
  });
  view.on('unlock', async () => {
    await readSlotsNow();
    drawAll();
  });
  view.on('online', () => read());
  let soon = null;
  view.on('change', () => {
    clearTimeout(soon);
    soon = setTimeout(read, 800);
  });
  view.signal.addEventListener('abort', () => clearTimeout(soon), { once: true });

  await read();
  const wanted = globalThis.location?.hash?.slice(1);
  if (wanted) ctx.show(wanted);
}
