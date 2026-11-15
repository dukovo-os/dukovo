// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The buttons that back up, check and restore a backup place (design 5.7 and
// 5.9). Each is a task, so a button that began one shows how far it is,
// "Backing up… 41%", and opens the task when it is pressed; the Now column of
// the table says the same. The page keeps one of these for all its buttons:
// the banner's, the table's and each place's. Which tasks run comes from
// the list of tasks when the page opens and from the task events after;
// words change in place, so that a button in hand stays in hand.

import { fill, h } from '../../dom.js';
import { jobWords, makeJobs } from './model.js';

/** The action each kind of button runs. */
const ACTIONS = Object.freeze({ run: 'backups.run', verify: 'backups.verify', restore: 'backups.restore' });

/** What the shell says of each kind, for the place `name`. */
const WORDS = Object.freeze({
  run: (name) => ({ unlock: `back up to ${name}`, running: `Backing up to ${name}`, done: 'The backup is done.', busy: 'Starting…' }),
  verify: (name) => ({ unlock: `check ${name}'s backups`, running: `Checking ${name}'s backups by restoring them`, done: 'The check is done.', busy: 'Starting…' }),
  restore: (name) => ({ unlock: `restore from ${name}`, running: `Restoring from ${name}`, done: 'The restore is done.', busy: 'Starting…' }),
});

/**
 * `ended(job, state)` is called when a task of a place ended, so that the
 * page reads the places again and, for a restore that is done, shows what it
 * brought. `repaint()` is called when the state of a job changed, for the
 * places that say it in other words than a button. Gives `{lock, button,
 * start, started, paint, load, ofPlace}`.
 */
export function makeJobsUi(view, { ended, repaint }) {
  const jobs = makeJobs();
  let locked = new Set();

  /** The job of the place `name`: a task, or the round of backups that holds the lock. */
  const ofPlace = (name) => jobs.of(name) ?? (locked.has(name) ? { id: null, kind: 'run', name, args: null, percent: null, step: null, mine: false } : null);

  /** Puts the words that tell the state of each button and each cell of the page. */
  function paint() {
    for (const button of view.main.querySelectorAll('[data-job]')) {
      if (button.getAttribute('aria-busy') === 'true') continue;
      const { job: kind, name, label } = button.dataset;
      const job = ofPlace(name);
      const own = job?.kind === kind ? job : null;
      const words = own ? jobWords(own) : label;
      if (button.dataset.shown === words) continue;
      button.dataset.shown = words;
      fill(button, words, own?.id != null && h('span', { class: 'sr-only' }, ' Opens the task.'));
    }
    for (const cell of view.main.querySelectorAll('[data-now]')) {
      const job = ofPlace(cell.dataset.now);
      const words = job ? jobWords(job) : cell.dataset.words;
      if (cell.textContent !== words) cell.textContent = words;
      cell.className = `st ${job ? 'off' : cell.dataset.tone}`.trim();
    }
    repaint();
  }

  /** Reads the list of tasks, which says which run now. */
  async function load() {
    try {
      jobs.learn((await view.api.get('/api/1/tasks'))?.tasks);
      paint();
    } catch (error) {
      // The tasks are the Tasks list's to show; without them a button says what it always says.
      if (error?.name === 'AbortError') return;
    }
  }

  /** Reads one task whole: an event holds no arguments, and the place is one. */
  async function learn(id) {
    try {
      jobs.put(await view.api.get(`/api/1/tasks/${id}`));
      paint();
    } catch {
      // The next list has it.
    }
  }

  view.on('task', (data) => {
    const found = jobs.event(data);
    if (found.learn !== undefined) learn(found.learn);
    paint();
    if (found.ended !== undefined) ended(found.ended, found.state);
  });
  view.on('online', load);

  /** Runs `kind` for `args` (`{name, ...}`) and keeps the task it began; `where` is where a refusal is shown. Gives the runner's answer. */
  async function start(kind, args, { button = null, words = null, where = {} } = {}) {
    view.slot.replaceChildren();
    const result = await view.api.run(ACTIONS[kind], args, { words: words ?? WORDS[kind](args.name), button, where });
    if (typeof result.task === 'number') jobs.started(result.task, ACTIONS[kind], args);
    paint();
    return result;
  }

  /** What a press does: begins the task, or opens the one that runs. */
  async function press(button) {
    const { job: kind, name } = button.dataset;
    const running = ofPlace(name);
    if (running?.kind === kind) {
      if (running.id === null) view.notice('A round of backups is running. It ends by itself.');
      else view.openTask(running.id);
      return;
    }
    await start(kind, { name }, { button });
  }

  return {
    ofPlace,
    load,
    paint,
    start,
    /** Keeps a task that began elsewhere on the page (the first backup after a place was added). */
    started(id, action, args) {
      jobs.started(id, action, args);
      paint();
    },
    /** The places whose round of backups holds the lock, `backup list`'s `running`, for a round that has no task of this page's. */
    lock(names) {
      locked = new Set(names);
    },
    /** A button of `kind` (`run`, `verify`) for the place `name`, `label` when none runs, with the command it runs under it when Show commands is on. */
    button(kind, name, label, { size = '' } = {}) {
      const button = h('button', { class: `btn${size ? ` ${size}` : ''}`, type: 'button', 'data-job': kind, 'data-name': name, 'data-label': label, 'data-shown': label }, label);
      button.addEventListener('click', () => press(button));
      const job = ofPlace(name);
      if (job?.kind === kind) {
        const words = jobWords(job);
        button.dataset.shown = words;
        fill(button, words, job.id !== null && h('span', { class: 'sr-only' }, ' Opens the task.'));
      }
      return h('div', { class: 'snap-action' }, button, view.api.inline(ACTIONS[kind], { name }));
    },
  };
}
