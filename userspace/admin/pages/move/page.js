// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Move to new hardware (design 6): /move takes a machine's backups to this
// machine in four steps. Find the backup (a plugged-in disk, a backup place
// or a folder), unlock it with the recovery key, check what changes, and
// restart to finish. `?from=NAME` (a backup place) or `?source=FOLDER` makes
// the choice of the first step, and `?via=setup` says the person came from
// Finish setting up. The page keeps what it is told in one object, `state`,
// which the four steps draw; a recovery key typed here lives in its field
// and nowhere else, and goes in the body of the call that needs it. The page
// is for administrators; the disks are read once the console is unlocked.

import { h, icon } from '../../dom.js';
import { PAGES } from '../../pages.js';
import { askToUnlock, isUnlocked } from '../../unlock.js';
import { FOLDER, makeFind } from './find.js';
import {
  argsOf, cardLines, chosenByEntry, crumbsOf, currentStep, defaultKey, defaultPicks, entryOf, keysOf, leadOf, sourcesOf, stepsOf,
} from './model.js';
import { makeCheck, makeKeyStep } from './plan.js';
import { makeStage } from './stage.js';

/** Whether the page with the id `id` is in this console. */
const hasPage = (id) => PAGES.some((entry) => entry.id === id && entry.load !== null);

export async function show(view) {
  const entry = entryOf(view.query);
  const links = { storage: hasPage('storage') };
  const state = {
    found: null,
    backups: null,
    keys: [],
    choice: null,
    file: null,
    used: null,
    plan: null,
    picks: {},
    reviewed: false,
    staged: null,
    task: null,
    failed: '',
    locked: false,
    reading: false,
    current: 'find',
  };
  let latestPlan = 0;

  view.head({ title: 'Move to new hardware', crumbs: crumbsOf(entry), commands: true });

  /** Asks an administrator whose console is locked to unlock, and says whether it is. */
  const unlockFor = async (words) => !view.session.admin || isUnlocked() || askToUnlock(view.session, words);

  const find = makeFind(view, state, {
    next: (choice) => choose(choice),
    look: () => readFound(),
    async unlock() {
      if (await unlockFor('look for backups on the plugged-in disks')) readFound();
    },
  });
  const key = makeKeyStep(view, state, { unlockFor, planned });
  const check = makeCheck(view, state, { pick, next: goOn }, { links, port: location.port });
  const stage = makeStage(view, state, {
    unlockFor, changed: () => paint(), staged: nowStaged, cancelled, stagedAlready,
  }, { port: location.port });
  const panels = { find, unlock: key, check, restart: stage };

  const lead = h('p', { class: 'muted' }, leadOf(null));
  const list = h('ol', { class: 'mv-list', 'aria-label': 'Steps' });
  const holders = Object.fromEntries(Object.entries(panels).map(([id, panel]) => [id, h('div', { class: 'mv-panel', 'data-step': id }, panel.element)]));
  view.main.append(h('div', { class: 'mv' },
    lead,
    h('div', { class: 'mv-wiz' },
      h('div', { class: 'card mv-steps' }, list),
      h('div', { class: 'mv-panels' }, ...Object.values(holders)))));

  // ---- the steps ----

  /** Draws the list of steps, shows the one that is open and says whose machine is moved. */
  function paint() {
    const steps = stepsOf({
      choice: state.choice, key: state.used, plan: state.plan, reviewed: state.reviewed, staged: Boolean(state.staged), staging: state.task !== null,
    });
    state.current = currentStep(steps, state.current);
    list.replaceChildren(...steps.map((step, at) => {
      const current = step.id === state.current;
      const dot = h('span', { class: 'mv-dot' }, step.done ? [icon('check'), h('span', { class: 'sr-only' }, 'Done: ')] : String(at + 1));
      const button = h('button', {
        class: `mv-step${step.done ? ' done' : ''}${current ? ' cur' : ''}`, type: 'button', 'aria-current': current ? 'step' : null, disabled: !step.reachable,
      }, dot, h('span', {}, step.title, step.summary && [h('br'), h('small', { class: 'muted' }, step.summary)]));
      button.addEventListener('click', () => go(step.id));
      return h('li', {}, button);
    }));
    for (const [id, holder] of Object.entries(holders)) holder.hidden = id !== state.current;
    lead.textContent = leadOf(state.plan?.machine ?? state.choice?.machine ?? null);
    stage.draw();
  }

  /** Opens `id` and puts the focus on its heading, so that a screen reader says where it is. */
  function go(id) {
    state.current = id;
    paint();
    panels[state.current].focus();
  }

  /** Forgets the plan and what went on from it, when the backup it was read from is not the one chosen. */
  function resetPlan() {
    latestPlan += 1;
    Object.assign(state, {
      plan: null, picks: {}, reviewed: false, used: null, failed: '',
    });
  }

  /** Step 1 went on with `choice`. */
  function choose(choice) {
    if (state.choice === null || state.choice.id !== choice.id || state.choice.source !== choice.source) resetPlan();
    state.choice = choice;
    state.file = defaultKey(state.keys, choice);
    key.draw();
    go('unlock');
  }

  /** Step 2 unlocked the backups: `plan` is what `move.plan` read, `how` the key that opened it. */
  function planned(plan, how) {
    state.plan = plan;
    state.picks = defaultPicks(plan);
    state.reviewed = false;
    state.failed = '';
    state.used = how === 'typed' ? { how, path: null, title: 'Recovery key typed' } : { how, path: state.file.path, title: state.file.title };
    if (plan.staged === true && !state.staged) state.staged = true;
    check.draw();
    go('check');
  }

  /** Step 3 went on. */
  function goOn() {
    state.reviewed = true;
    go('restart');
  }

  /**
   * A card's address was chosen. With a key file the plan is read again with
   * the choices, so that what it says of the firewall follows them; a typed
   * key is gone from the page, so the plan stays and the choices go to Stage
   * it as they are.
   */
  async function pick(mac, value) {
    state.picks = { ...state.picks, [mac]: value };
    check.refresh();
    if (state.used?.how !== 'file') return;
    latestPlan += 1;
    const mine = latestPlan;
    const args = argsOf({ source: state.choice.source, file: state.used.path, cards: cardLines(state.plan, state.picks) });
    const done = await view.api.run('move.plan', args, { words: { unlock: 'read the backups again', busy: 'Reading…' } });
    if (!done.ok || mine !== latestPlan || view.signal.aborted) return;
    const same = (plan) => JSON.stringify([plan.firewall_on_missing, plan.cards]);
    if (same(done.result) === same(state.plan)) return;
    state.plan = done.result;
    check.draw();
    check.focusChoice(mac, value);
  }

  // ---- what is staged ----

  /** Reads the backup places, which say whether a restore is staged and which places this machine has. */
  async function readBackups() {
    try {
      state.backups = await view.api.get('/api/1/backups');
      if (state.backups?.staged) state.staged = state.backups.staged;
    } catch (error) {
      // The places are a convenience; the disks and a folder are still there to choose from.
      if (error?.name === 'AbortError') return;
    }
  }

  /** The task that stages ended well: what is staged is read, so that the words say where from. */
  async function nowStaged() {
    state.task = null;
    state.staged = true;
    await readBackups();
    paint();
    stage.focus();
  }

  /** Cancel did its work. */
  function cancelled() {
    state.staged = null;
    state.task = null;
    paint();
    stage.focus();
  }

  /** The task that stages failed because one is staged already (BAK-104): that one is shown. */
  async function stagedAlready() {
    state.staged = true;
    await readBackups();
    go('restart');
  }

  // ---- reading the disks ----

  /** The backup the address chose, once there is a list to choose it from. */
  function adoptEntry() {
    if (state.choice !== null) return;
    const { disks, places } = sourcesOf(state.found, state.backups, view.time);
    const found = chosenByEntry([...disks, ...places], entry);
    if (found !== null) {
      state.choice = found;
    } else if (entry.source !== null) {
      state.choice = {
        id: FOLDER, kind: 'folder', source: entry.source, machine: null, title: 'A folder', detail: entry.source,
      };
    }
  }

  /** Looks for backups and key files on the plugged-in disks, when the console is unlocked. */
  async function readFound() {
    state.locked = view.session.admin && !isUnlocked();
    if (state.locked || state.reading) {
      find.draw();
      return;
    }
    state.reading = true;
    find.draw();
    const done = await view.api.run('move.find', {}, { words: { unlock: 'look for backups on the plugged-in disks' } }).catch(() => ({ ok: false }));
    if (view.signal.aborted) return;
    state.reading = false;
    if (done.ok) {
      state.found = done.result;
      state.keys = keysOf(done.result);
      // A folder that was typed and is a disk that was found is that disk, which says whose backups it holds.
      const disk = state.choice?.kind === 'folder' ? sourcesOf(done.result, null, view.time).disks.find((one) => one.source === state.choice.source) : null;
      if (disk) state.choice = disk;
      adoptEntry();
      state.file ??= state.choice ? defaultKey(state.keys, state.choice) : null;
    }
    find.draw();
    key.draw();
    paint();
  }

  view.on('unlock', () => {
    state.locked = view.session.admin && !isUnlocked();
    if (!state.locked && state.found === null) readFound();
    else find.draw();
  });

  await readBackups();
  adoptEntry();
  state.file = state.choice ? defaultKey(state.keys, state.choice) : null;
  if (state.staged) state.current = 'restart';
  else if (state.choice !== null) state.current = 'unlock';
  find.draw();
  key.draw();
  paint();
  await Promise.all([readFound(), stage.load()]);
}
