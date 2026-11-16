// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Step 4 of Move to new hardware (design 6.3 and 6.4): restart to finish.
// Stage it runs `move.stage`, a task that receives the files into a place
// that the machine starts from, as long as reading them takes; nothing on
// the machine changes until it restarts, and Cancel (`move.cancel`) takes
// what was staged away until then. Restart now asks first, as the Hardware
// page does, and counts the minute on the card; the restart is also
// `elevate reboot`, in a box to copy. A restore that is staged already, by a terminal or another tab, is shown as staged, with Cancel and
// Stage it off. A typed key is asked for again here, and is sent only in the
// body of the call.

import {
  fill, h, icon, uniqueId,
} from '../../dom.js';
import { makeSecretField } from '../../lib/secret-field.js';
import { endWords, percentOf, stepWords } from '../../lib/tasks-words.js';
import { copyBox } from '../snapshots/copy-box.js';
import {
  afterOf, argsOf, cardLines, RESTART_COMMAND, STAGED_WORDS, stageFault,
} from './model.js';

/** The action that stages the restore, and the one that cancels it. */
const STAGE = 'move.stage';
const CANCEL = 'move.cancel';

/** Whether a task of the list or an event is still to run or running. */
const running = (task) => task.state === 'waiting' || task.state === 'running';

/**
 * Whether a task that stages failed because a restore was staged already,
 * by a terminal or another tab: `backup restore --machine` ends its words
 * with the error's ID (BAK-104), and the task's note is its last line.
 */
const stagedFirst = (task) => task.state === 'failed' && typeof task.note === 'string' && task.note.includes('(BAK-104)');

/**
 * Step 4 in a card. `state` is the page's: `{plan, picks, used, choice,
 * staged, task, failed}`, where `staged` is null, true, or `{from, at}`, and
 * `task` the progress of the task that stages, `{id, state, step, steps,
 * words, percent}`. `hooks.unlockFor(words)` asks an administrator whose
 * console is locked to unlock; `hooks.changed()` is called when the page
 * is to be drawn again, `hooks.staged()` when the task ended well, and
 * `hooks.cancelled()` when Cancel did its work; `hooks.stagedAlready()` when
 * the task failed because a restore is staged already (BAK-104). `port`
 * is the console's. Gives `{element, focus, draw, load}`: `draw()` shows the
 * state as it is, `load()` finds a task of this action that runs already.
 */
export function makeStage(view, state, hooks, { port = '' } = {}) {
  const id = uniqueId('mv-stage');
  const heading = h('h2', { id: `${id}-h`, tabindex: '-1' }, 'Restart to finish');
  const body = h('div', { class: 'mv-stage' });
  const errors = h('div', { class: 'mv-errors' });
  const pendingHost = h('div', { class: 'mv-pending' });
  let field = null;
  let shown = null;
  let progress = null;
  let line = null;

  /** The words and the bar of the task that stages; they change in place, so that nothing in hand is replaced. */
  function paintProgress() {
    if (progress === null || state.task === null) return;
    const percent = percentOf(state.task);
    progress.words.textContent = stepWords(state.task);
    progress.meter.hidden = percent === null;
    progress.meter.style?.setProperty('--v', String(percent ?? 0));
    progress.meter.setAttribute('aria-label', `${percent ?? 0} percent`);
  }

  async function stage(button) {
    errors.replaceChildren();
    if (!(await hooks.unlockFor('stage the restore'))) return;
    const typed = state.used?.how === 'typed' ? field.take() : '';
    if (typed === null) return;
    const args = argsOf({
      source: state.choice.source,
      file: state.used?.how === 'file' ? state.used.path : null,
      typed,
      cards: cardLines(state.plan, state.picks),
      at: state.plan.at ?? null,
    });
    const done = await view.api.run(STAGE, args, {
      words: { unlock: 'stage the restore', running: `Staging the restore from ${state.plan.machine}`, busy: 'Staging…' }, button, where: { slot: errors },
    });
    if (view.signal.aborted) return;
    if (done.ok && typeof done.task === 'number') {
      state.failed = '';
      state.task = {
        id: done.task, state: 'running', step: 0, steps: 0, words: '', percent: null,
      };
      hooks.changed();
    }
  }

  async function cancel(button) {
    errors.replaceChildren();
    const done = await view.api.run(CANCEL, {}, {
      words: { unlock: 'cancel the staged restore', done: 'The staged restore is cancelled.', unchanged: 'Nothing was staged.', busy: 'Cancelling…' }, button, where: { slot: errors },
    });
    if (!view.signal.aborted && done.ok) hooks.cancelled();
  }

  /** The line that counts the minute, in the card; it asks `power.status` the first time, for a restart that is set already. */
  async function pending() {
    if (line === null) {
      const { pendingRestart } = await import('../../lib/pending-restart.js');
      line = pendingRestart(view);
      pendingHost.append(line.element);
      line.read();
    }
    return line;
  }

  /** Asks, then restarts the machine in a minute. */
  async function restart() {
    const { restartNow } = await import('../../lib/restart.js');
    const from = typeof state.staged?.from === 'string' ? ` from ${state.staged.from}` : '';
    await restartNow(view, { why: `To finish the restore${from}.`, pending: await pending() });
  }

  /** What step 4 shows once a restore is staged: the words, the restart's command, Restart now and Cancel. */
  function stagedView() {
    const from = typeof state.staged?.from === 'string' ? ` from ${state.staged.from}` : '';
    const restartButton = h('button', { class: 'btn default', type: 'button' }, 'Restart now…');
    restartButton.addEventListener('click', restart);
    const cancelButton = h('button', { class: 'btn', type: 'button' }, 'Cancel');
    cancelButton.addEventListener('click', () => cancel(cancelButton));
    pending();
    return [
      h('div', { class: 'banner ok', role: 'status' }, icon('check'), h('div', {}, h('p', {}, h('b', {}, `Staged${from}.`), ` ${STAGED_WORDS}`))),
      pendingHost,
      h('p', {}, 'To restart this machine and finish, press Restart now, or type this at a terminal:'),
      copyBox(RESTART_COMMAND, 'The command that restarts this machine'),
      state.plan !== null && h('p', {}, afterOf(state.plan, state.picks, { port }).open),
      h('div', { class: 'btns' }, restartButton, cancelButton),
    ];
  }

  /** What step 4 shows while the task that stages runs: how far it is, and a way to the task. */
  function runningView() {
    const meter = h('div', { class: 'meter', role: 'img' }, h('i'));
    progress = { words: h('p', { role: 'status' }), meter };
    const open = h('button', { class: 'btn small', type: 'button' }, 'Open the task');
    open.addEventListener('click', () => view.openTask(state.task.id));
    return [progress.words, meter, h('div', { class: 'btns' }, open)];
  }

  /** What step 4 shows before anything is staged: what Stage it does, the key again when it was typed, and why it is off when it is. */
  function idleView() {
    const fault = stageFault(state.plan, state.picks);
    const button = h('button', { class: 'btn default', type: 'button', 'aria-disabled': String(fault !== '') }, 'Stage it');
    const reason = h('p', { class: 'msg', role: 'status' }, fault);
    button.addEventListener('click', () => {
      if (fault !== '') reason.textContent = fault;
      else stage(button);
    });
    field = state.used?.how === 'typed'
      ? makeSecretField({
        label: 'Recovery key', kind: 'key', hint: 'Type the key again to stage it. It is not kept.', signal: view.signal,
      })
      : null;
    return [
      h('p', { class: 'muted' }, `Stage it reads ${state.plan.machine}'s files into a place that this machine starts from. It takes as long as reading the files takes, and nothing changes until this machine restarts.`),
      state.failed !== '' && h('div', { class: 'banner bad', role: 'status' }, icon('warn'), h('div', {}, h('p', {}, state.failed))),
      field?.element,
      reason,
      h('div', { class: 'btns' }, button),
    ];
  }

  /** The state of the card: staged, being staged, or ready. A card with a field in it is drawn again only when what it shows changes. */
  function draw() {
    const mode = state.staged ? 'staged' : state.task !== null ? 'running' : 'idle';
    if (mode === 'idle' && state.plan === null) {
      shown = null;
      fill(body);
      return;
    }
    const signature = JSON.stringify([mode, state.used?.how, state.failed, state.plan?.machine, stageFault(state.plan ?? {}, state.picks)]);
    if (signature !== shown) {
      shown = signature;
      progress = null;
      field = null;
      const content = { staged: stagedView, running: runningView, idle: idleView }[mode];
      fill(body, h('section', { class: 'card', 'aria-labelledby': heading.id }, heading, ...content(), errors));
    }
    paintProgress();
  }

  /** Follows the task `data` (a task of the list or an event) when it is the one that stages. */
  function follow(data) {
    if (state.task === null || data?.id !== state.task.id) return;
    if (running(data)) {
      state.task = { ...state.task, ...data };
    } else if (data.state === 'done') {
      state.task = null;
      hooks.staged();
      return;
    } else if (stagedFirst(data)) {
      // The one staged first is as it was, and is shown, with Cancel.
      state.task = null;
      hooks.stagedAlready();
      return;
    } else {
      state.task = null;
      state.failed = `${endWords(data)}. Nothing is staged.`;
    }
    hooks.changed();
  }
  view.on('task', follow);

  /** Finds a task that stages and runs already: this page began it in another tab, or a terminal did. */
  async function load() {
    try {
      const found = ((await view.api.get('/api/1/tasks'))?.tasks ?? []).find((task) => task.action === STAGE && running(task));
      if (found && state.task === null) {
        state.task = { ...found };
        hooks.changed();
      }
    } catch (error) {
      // The Tasks drawer has the list; without it, Stage it is as it is.
      if (error?.name === 'AbortError') return;
    }
  }

  return { element: body, focus: () => heading.focus(), draw, load };
}
