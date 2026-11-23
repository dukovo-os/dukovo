// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Hardware: what this machine is, its last check and the check itself, the
// temperatures and fans, the disks and their own tests, the memory test, the
// devices, and restarting or shutting it down. The page reads what it shows
// when it opens and again when a check of its own ends; the memory's modules
// and the disks' own tests only for an administrator whose console is
// unlocked. The temperatures are read apart from the rest, since the cards do
// not wait for them, and again every minute. A machine with a BMC, which an
// administrator whose console is unlocked sees named in `hardware show`, has a
// BMC card of its own, and its temperatures and fans join the temperatures'.
// The dialog that checks, the dialog that asks for the memory test, the
// restart's dialogs and the list of devices are read when a button asks.

import { fill, h } from '../../dom.js';
import { stepWords } from '../../lib/tasks-words.js';
import { askToUnlock, isUnlocked } from '../../unlock.js';
import { button, makeCard, setOff } from './cards.js';
import { makeCheck } from './check.js';
import { makeDisks } from './disks.js';
import { makeMachine } from './machine.js';
import { makeMemory } from './memory.js';
import { MEMTEST_RESTART_WHY, quietMinutes } from './memtest-model.js';
import {
  diskNotes, endedCheck, hasResult, summaryWords,
} from './model.js';
import { makePower } from './power.js';

const READS = {
  machine: '/api/1/hardware',
  last: '/api/1/hardware/check',
  history: '/api/1/hardware/history',
  about: '/api/1/console/about',
  storage: '/api/1/storage',
  tasks: '/api/1/tasks',
  memtest: '/api/1/hardware/memtest',
};
const TEMPS = { sensors: '/api/1/hardware/sensors', highest: '/api/1/hardware/highest', updates: '/api/1/updates/history' };
// What an administrator reads of a BMC: the same sensors with the BMC's own, and its log.
const BMC_SENSORS = '/api/1/hardware/bmc/sensors';
const BMC_EVENTS = '/api/1/hardware/bmc/events';
const TEMPS_MS = 60_000;
const CHECK = 'hardware.check';
const running = (task) => task.state === 'running' || task.state === 'waiting';

export async function show(view) {
  const admin = view.session.admin === true;
  // `last` is null when no check was kept, and undefined when it could not be read.
  const data = {
    machine: null, inventory: null, access: admin ? 'locked' : 'reader', about: null, last: null, history: null, storage: null, ended: null,
    // The memory test at the start: its answer, or undefined when it could not be read.
    memtest: null,
    // The temperatures: null until read, undefined when they could not be.
    sensors: null, highest: null, updates: null,
    // The BMC's event log: null until read, undefined when it could not be.
    events: null,
  };
  const tasks = new Map();
  let line = null;
  let latest = 0;
  let latestTemps = 0;
  let loaded = false;

  const lead = h('p', { id: 'hw-lead', class: 'hw-lead' }, 'Reading the hardware…');
  const say = h('p', { class: 'sr-only', role: 'status' });
  const checkNow = button('Check the hardware', () => ctx.openCheck());
  const ctx = {
    view,
    admin,
    data,
    pendingHost: null,
    /** The check that runs, or with `disk` the one that runs on that disk; null for none. */
    running: (disk) => [...tasks.values()].find((one) => running(one) && (disk === undefined || one.args?.disk === disk)) ?? null,
    say: (words) => { say.textContent = words; },
    show(id) {
      const target = view.main.querySelector(`#${id}`);
      target?.scrollIntoView?.();
      target?.querySelector('h2')?.focus();
    },
    /** Starts a check; gives whether it started. `slot` is where a refusal goes. */
    async startCheck(args, one, slot) {
      fill(slot);
      const result = await view.api.run(CHECK, args, {
        button: one,
        where: { slot },
        words: {
          unlock: 'check the hardware', running: 'Checking the hardware', busy: 'Starting the check…', done: 'The check is done.',
        },
      });
      if (typeof result.task === 'number') {
        data.ended = null;
        tasks.set(result.task, {
          id: result.task, action: CHECK, args, state: 'running', step: 0, steps: 0,
        });
        say.textContent = 'The check started.';
        drawAll();
      }
      return result.ok === true;
    },
    async openCheck() {
      if (ctx.running() !== null) return;
      const { openCheckDialog } = await import('./check-dialog.js');
      await openCheckDialog(ctx);
    },
    /** Reads the modules, asking an administrator to unlock first; gives whether they were read. */
    async readInventory() {
      const read = await readInventory(true);
      if (read) drawAll();
      return read;
    },
    async pending() {
      if (line === null) {
        const { pendingRestart } = await import('../../lib/pending-restart.js');
        line = pendingRestart(view);
        (ctx.pendingHost ?? view.slot).append(line.element);
      }
      return line;
    },
    /** Reads whether the memory test is asked for, and draws the card again. */
    async readMemtest() {
      try {
        data.memtest = await view.api.get(READS.memtest);
      } catch (error) {
        view.problem(error, { slot: memory.errors });
        return;
      }
      memory.draw();
    },
    /** Asks for the memory test or cancels it, as `name` of memtest-dialog.js says. */
    memtest: async (name, one, slot) => (await import('./memtest-dialog.js'))[name](ctx, one, slot),
    /** Restarts for the memory test, which the shell waits longer for than for any other restart. */
    restartToTest: () => ctx.power('restart', MEMTEST_RESTART_WHY, { kind: 'memtest', quiet_minutes: quietMinutes(data.machine?.memory?.bytes) }),
    async power(name, ...args) {
      const lib = await import('../../lib/restart.js');
      const pending = await ctx.pending();
      if (name === 'restart') await lib.restartNow(view, { why: args[0], pending, ...args[1] });
      else if (name === 'restartAt') await lib.restartAt(view, args[0], { pending });
      else await lib.shutDown(view, { pending });
    },
  };

  const machine = makeMachine(ctx);
  const check = makeCheck(ctx);
  // The temperatures' own module comes with their first answer, which is read apart from the rest.
  const tempsCard = makeCard('temps', 'Temperatures and fans');
  fill(tempsCard.body, h('p', { class: 'muted' }, 'Reading the temperatures…'));
  let temps = null;
  const devicesCard = makeCard('devices', 'Devices');
  fill(devicesCard.body,
    h('p', {}, 'Every card and port the system can tell of, with the driver that took it. The list is read when you ask.'),
    h('div', { class: 'btns' }, button('Show the devices', async (one) => (await import('./devices.js')).drawDevices(ctx, devicesCard, one))));
  // The BMC card's own module comes when the page finds the machine has a BMC.
  let bmc = null;
  const disks = makeDisks(ctx);
  const memory = makeMemory(ctx);
  const power = makePower(ctx);
  ctx.pendingHost = power.host;

  // The cards come in when the first read is in, so that none says "Not checked yet" before it knows.
  const order = [machine, check, tempsCard, disks, memory, devicesCard, power].map((one) => one.element);
  let cards = order;
  tempsCard.element.classList.add('hw-wide');
  devicesCard.element.classList.add('hw-wide');
  power.element.classList.add('hw-wide');
  view.head({ title: 'Hardware', commands: true, buttons: admin ? [checkNow] : [] });
  const host = h('div', { class: 'hw' }, lead, !admin && h('p', { class: 'muted' }, 'An administrator can check the hardware and restart or shut down this machine.'), say);
  view.main.append(host);

  function drawAll() {
    const task = ctx.running();
    setOff(checkNow, task !== null);
    if (task !== null) checkNow.setAttribute('aria-describedby', lead.id);
    else checkNow.removeAttribute('aria-describedby');
    if (task) {
      fill(lead, `Checking the hardware. ${stepWords(task)}`);
    } else if (loaded) {
      const health = diskNotes(data.storage);
      fill(lead,
        hasResult(data.last) && summaryWords(data.last, view.time),
        health && [hasResult(data.last) && ' ', health.lead, health.notes.flatMap((note) => ['; ', h('a', { href: note.href }, note.words)]), '.']);
    }
    for (const one of [machine, check, disks, memory]) one.draw();
  }

  /** Reads the modules; `ask` lets a locked console be asked to unlock. Gives whether they were read. */
  async function readInventory(ask) {
    if (!admin) return false;
    if (!isUnlocked() && !(ask && await askToUnlock(view.session, 'read the memory modules'))) return false;
    try {
      data.inventory = await view.api.get('/api/1/hardware/inventory');
      data.access = 'ok';
      await readBmc();
      return true;
    } catch (error) {
      if (error?.id !== 'ADM-106' && error?.id !== 'ADM-122') view.problem(error, { slot: machine.errors });
      return false;
    }
  }

  /** Reads the BMC's log, if the machine has a BMC, and shows its card with the sensors read so far; the temperatures are read again for the BMC's own. */
  async function readBmc() {
    if (!data.inventory?.bmc) return;
    if (bmc === null) {
      bmc = (await import('./bmc.js')).makeBmc(ctx);
      bmc.element.classList.add('hw-wide');
      // The card is between the temperatures and the disks; appending moves each card to its place.
      order.splice(3, 0, bmc.element);
      if (cards === null) host.append(...order);
    }
    await readEvents();
    if (data.sensors !== null) readTemps(['sensors']);
  }

  async function readEvents() {
    try {
      data.events = await view.api.get(BMC_EVENTS);
    } catch {
      data.events = undefined;
    }
    bmc.draw();
  }

  /** Reads the temperatures, `names` of `TEMPS`, the sensors first; one that cannot be read leaves what was read before. Only the last read to ask is drawn. */
  async function readTemps(names) {
    latestTemps += 1;
    const mine = latestTemps;
    // An administrator reads the sensors with the BMC's, and those without them when the console has locked since.
    const ask = (name) => (name === 'sensors' && data.inventory?.bmc ? view.api.get(BMC_SENSORS).catch(() => view.api.get(TEMPS[name])) : view.api.get(TEMPS[name]));
    const [module, ...found] = await Promise.allSettled([import('./sensors.js'), ...names.map(ask)]);
    if (mine !== latestTemps || view.signal.aborted) return;
    const first = data.sensors === null;
    names.forEach((name, at) => {
      if (found[at].status === 'fulfilled') data[name] = found[at].value;
      else if (data[name] === null) data[name] = undefined;
    });
    const failed = [module, found[0]].find((one) => one.status === 'rejected');
    if (failed === undefined) fill(tempsCard.errors);
    else if (first) view.problem(failed.reason, { slot: tempsCard.errors });
    if (module.status === 'fulfilled') {
      temps ??= module.value.makeTemps(ctx, tempsCard);
      temps.draw();
      bmc?.draw();
    }
  }

  /** Reads everything again; only the last read to ask is drawn. */
  async function read() {
    latest += 1;
    const mine = latest;
    const names = Object.keys(READS);
    const found = await Promise.allSettled(names.map((name) => view.api.get(READS[name])));
    if (mine !== latest || view.signal.aborted) return;
    const got = Object.fromEntries(names.map((name, at) => [name, found[at].status === 'fulfilled' ? found[at].value : undefined]));
    data.machine = got.machine ?? null;
    data.last = got.last;
    data.history = Array.isArray(got.history) ? got.history : null;
    data.about = got.about ?? null;
    data.storage = got.storage ?? null;
    data.memtest = got.memtest;
    if (Array.isArray(got.tasks?.tasks)) {
      tasks.clear();
      for (const task of got.tasks.tasks) if (task.action === CHECK && running(task)) tasks.set(task.id, task);
      data.ended = endedCheck(got.tasks.tasks, data.last);
    }
    if (admin && isUnlocked() && data.access !== 'ok') await readInventory(false);
    if (mine !== latest || view.signal.aborted) return;
    const first = !loaded;
    loaded = true;
    if (cards !== null) {
      host.append(...cards);
      cards = null;
      power.draw();
    }
    drawAll();
    temps?.draw();
    disks.read(false);
    if (first) readTemps(Object.keys(TEMPS));
    for (const name of ['machine', 'last']) if (found[names.indexOf(name)].status === 'rejected') view.problem(found[names.indexOf(name)].reason, { slot: name === 'machine' ? machine.errors : check.errors });
    const status = await view.api.get('/api/1/power').catch(() => null);
    if (mine !== latest || view.signal.aborted) return;
    if (status?.shutdown) (await ctx.pending()).set(status.shutdown);
    else if (line) line.set(null);
  }

  view.on('task', async (event) => {
    if (event.action !== CHECK && !tasks.has(event.id)) return;
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
    tasks.delete(event.id);
    say.textContent = task.state === 'done' ? 'The check is done.' : 'The check did not finish.';
    await read();
  });
  view.on('power', async (event) => {
    if (event?.shutdown) (await ctx.pending()).set(event.shutdown);
  });
  view.on('unlock', async () => {
    disks.read(false);
    if (data.access !== 'ok' && await readInventory(false)) drawAll();
  });
  view.on('online', () => read());
  view.every(TEMPS_MS, () => {
    if (data.inventory?.bmc && isUnlocked()) readEvents();
    return readTemps(['sensors']);
  });

  await read();
  const wanted = globalThis.location?.hash?.slice(1);
  if (wanted) ctx.show(wanted);
}
