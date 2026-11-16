// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The steps of a flow that changes the machine more than once: a list that runs
// actions one after the other, says each step's state in words as well as by
// mark, and stops at the first one that fails. What was done stays done: each
// command takes back its own half-made change, so nothing here undoes anything,
// and the step that failed says what stands. A step is an answer, which the
// call itself gives, or a task, which the Tasks stream follows to its end. The
// pages that run a flow read this file when a flow starts.

import { ApiError } from '../api.js';
import { fill, h, icon } from '../dom.js';
import { LOST_ANSWER } from './action-model.js';
import { isRunning } from './tasks-model.js';
import { endWords, stepWords } from './tasks-words.js';

/** How often a task that runs is read, in case its end was missed. */
const POLL_MS = 5000;

/** What each state of a step says, which the mark beside it does not say alone. */
const STATES = Object.freeze({
  waiting: 'Waiting', running: 'Running', done: 'Done', failed: 'Failed', skipped: 'Not run',
});

/** The mark of each state: the shell's round tick, filled or ringed. */
const MARKS = Object.freeze({
  waiting: 'tick', running: 'tick going', done: 'tick done', failed: 'tick bad', skipped: 'tick',
});

/** The last line of `lines` that says something, or empty text. */
const lastOf = (lines) => [...(Array.isArray(lines) ? lines : [])].reverse().find((line) => String(line).trim() !== '')?.trim() ?? '';

/** The error ID a line of a command's output ends with, as `(WEB-103)`, or null. */
const idOf = (line) => /\(([A-Z]{2,5}-\d{3})\)\s*$/.exec(line)?.[1] ?? null;

/** One step of the list: its row, the place a failure is drawn, and `show(state, words)`. */
function itemOf(step) {
  const mark = h('span', { 'aria-hidden': 'true' });
  const state = h('span', { class: 'flow-state small muted' });
  const line = h('p', { class: 'small muted flow-line', role: 'status' });
  const stands = h('p', { class: 'flow-stands', hidden: true });
  const problem = h('div', { class: 'flow-problem', tabindex: '-1' });
  const element = h('li', { class: 'flow-step' }, mark,
    h('div', { class: 'flow-body' }, h('div', { class: 'flow-head' }, h('b', {}, step.title), state), line, stands, problem));
  const item = {
    element,
    problem,
    show(kind, words = '') {
      element.setAttribute('data-state', kind);
      mark.setAttribute('class', MARKS[kind]);
      fill(mark, kind === 'done' && icon('check'), kind === 'failed' && icon('x'));
      state.textContent = STATES[kind];
      line.textContent = words;
    },
    /** Marks the step failed, says what stands and moves the focus to the message. */
    fail() {
      item.show('failed');
      if (step.words?.stands) {
        stands.textContent = step.words.stands;
        stands.hidden = false;
      }
      problem.focus();
    },
  };
  item.show('waiting');
  return item;
}

/** The task `id` once it has ended, read whole; `progress(event)` hears each event of a task that runs. Null when the page is left first. */
function endOf(view, id, progress) {
  return new Promise((resolve) => {
    if (view.signal.aborted) {
      resolve(null);
      return;
    }
    let over = false;
    const finish = (task) => {
      if (over) return;
      over = true;
      off();
      stop();
      resolve(task);
    };
    // An event holds no log, so the end is read; the read is also the way to
    // learn of an end whose event came before this listened.
    const read = async () => {
      try {
        const task = await view.api.get(`/api/1/tasks/${id}`);
        if (!isRunning(task)) finish(task);
        else progress(task);
      } catch {
        // The next event or read has it; a page that is left ends the wait below.
      }
    };
    const off = view.on('task', (event) => {
      if (event.id !== id) return;
      if (isRunning(event)) progress(event);
      else read();
    });
    const stop = view.every(POLL_MS, read);
    view.signal.addEventListener('abort', () => finish(null), { once: true });
    read();
  });
}

/** The error that stands for a task that did not end well, in the words of the command's last line. */
function taskFailure(step, task) {
  const lines = Array.isArray(task.log) ? task.log : [];
  const last = lastOf(lines);
  const id = idOf(last);
  return new ApiError({
    id,
    title: step.words?.failed ?? `${step.title} did not work`,
    detail: last === '' ? endWords(task) : last.replace(/\s*\([A-Z]{2,5}-\d{3}\)$/, ''),
    fix: step.words?.fix ?? '',
    status: 422,
    command: lines.length > 0 ? { argv: [], status: task.status ?? null, stderr: lines.join('\n') } : null,
  });
}

/**
 * Runs one step. Gives `{ok, answer, line}`: `answer` is what the call
 * answered or, for a task, the task as it ended, and `line` the last line of
 * its output; `ok` false has put the reason in the step's row. `answer` is
 * undefined for a step that was not sent, and null when the page was left.
 * A step that waits for a task but whose call answered at once (no task) is
 * done with that answer.
 */
async function runOne(view, step, item) {
  const ran = await view.api.run(step.action, step.args, { words: step.words, where: { slot: item.problem } });
  if (!ran.ok) {
    // The runner draws what it refused; a lost answer and an unlock that was
    // not given are its notice and its silence.
    if (item.problem.children.length === 0) {
      fill(item.problem, h('p', { class: 'msg' }, icon('warn'), h('span', {}, ran.lost ? LOST_ANSWER : 'It did not start, so nothing was changed.')));
    }
    return { ok: false };
  }
  let answer = ran.result;
  if (step.wait === 'task' && typeof ran.task === 'number') {
    answer = await endOf(view, ran.task, (task) => item.show('running', stepWords(task)));
    if (answer === null) return { ok: false, answer };
    if (answer.state !== 'done') {
      view.problem(taskFailure(step, answer), { slot: item.problem, buttons: [h('button', { class: 'btn small', type: 'button', onclick: () => view.openTask(ran.task) }, 'Open the task')] });
      return { ok: false, answer };
    }
  }
  if (step.ok && !step.ok(answer)) {
    view.problem(new ApiError({ title: `${step.title} did not give the answer it should`, detail: 'The server did not say that it was done.', status: 422 }), { slot: item.problem });
    return { ok: false, answer };
  }
  return { ok: true, answer, line: step.secret ? '' : lastOf(answer?.log ?? answer?.output) };
}

/**
 * Runs `steps` in order and draws them as a list. A step is `{title, action,
 * args, wait, ok, secret, words}`: `wait` is `answer` (done when the call
 * answers) or `task` (done when the task it starts ends); `ok(answer)`, when
 * given, says whether what came back is what the step needed; `secret` keeps
 * the step's answer, which holds a password, out of what is drawn; `words` is
 * the runner's (`unlock`, `running`, `done`) with `failed`, the title of the
 * error, and `stands`, what stands when this step fails.
 *
 * Gives `{element, finished}`: the list, and a promise of `{done, failed,
 * answers, left}`: whether every step is done, the index of the one that
 * failed or null, each step's answer (a task as it ended; none after the
 * failure) and whether the page was left first. No step is run after a
 * failure.
 */
export function runSteps(view, steps) {
  const items = steps.map(itemOf);
  const element = h('ol', { class: 'chk-list flow' }, ...items.map((item) => item.element));
  const finished = (async () => {
    const answers = [];
    for (const [at, step] of steps.entries()) {
      items[at].show('running');
      const outcome = await runOne(view, step, items[at]);
      if (view.signal.aborted) return { done: false, failed: null, answers, left: true };
      answers.push(outcome.answer);
      if (!outcome.ok) {
        items[at].fail();
        items.slice(at + 1).forEach((item) => item.show('skipped'));
        return { done: false, failed: at, answers, left: false };
      }
      items[at].show('done', outcome.line);
    }
    return { done: true, failed: null, answers, left: false };
  })();
  return { element, finished };
}
