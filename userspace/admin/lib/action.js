// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Running an action (design 7), which every page does the same way:
//
// 1. a reader gets ADM-122's words and no request; a locked administrator
//    is asked to unlock first ("Unlock to restart nginx");
// 2. the call, and when the lock fell between, the dialog and the same call
//    once more;
// 3. 202: a notice that the task runs, and the Tasks drawer learns it;
// 4. 200: a notice from the action's words and `changed`;
// 5. an error is put where it belongs by `problem`;
// 6. no answer: never sent again by itself.
//
// The button that ran it is busy until one of 3 to 6.

import {
  changeDone, findAction, gotNoAnswer, LOST_ANSWER, needsAdministrator, readerProblem, requestFor, taskStarted,
  wordsFor,
} from './action-model.js';

/** Puts `button` in its busy state with `words`; gives the function that puts it back. */
function busyButton(button, words) {
  if (!button) return () => {};
  const before = [...button.childNodes];
  button.setAttribute('aria-busy', 'true');
  button.setAttribute('aria-disabled', 'true');
  button.replaceChildren(words);
  return () => {
    button.removeAttribute('aria-busy');
    button.removeAttribute('aria-disabled');
    button.replaceChildren(...before);
  };
}

/**
 * `request(method, path, body)` is api.js's call with the page's signal;
 * `actions()` gives the list of `GET /api/1/actions`; `session()` the
 * session; `askToUnlock(session, words)` is unlock.js's; `notice`,
 * `problem` and `openTasks` are the shell's; `learnTask(id)` tells the
 * Tasks drawer of a task; `isOffline()` says whether the server answers.
 */
export function makeRunner({
  request, actions, session, askToUnlock, notice, problem, openTasks, learnTask, isOffline,
}) {
  const running = new WeakSet();

  /**
   * Runs the action `id` with `args`. `options`: `words` (see wordsFor),
   * `button` (the one that is busy), `where` (for `problem`). Gives
   * `{ok, result, task, cancelled}`.
   */
  return async function run(id, args = {}, { words: given, button, where = {} } = {}) {
    if (button && running.has(button)) return { ok: false, busy: true };
    const list = await actions().catch((error) => {
      problem(error, where);
      return null;
    });
    if (list === null) return { ok: false };
    const entry = findAction(list, id);
    if (!entry) {
      problem({
        id: null, title: `There is no action ${id}`, detail: 'This console does not have it.', fix: '', status: 404, field: null, command: null,
      }, where);
      return { ok: false };
    }
    const words = wordsFor(entry, given);
    if (isOffline()) {
      notice('The server is not answering. Nothing was done.', { kind: 'warn' });
      return { ok: false };
    }
    if (needsAdministrator(entry)) {
      const now = session();
      if (!now.admin) {
        problem(readerProblem(now), where);
        return { ok: false };
      }
      if (!(await askToUnlock(now, words.unlock))) return { ok: false, cancelled: true };
    }

    if (button) running.add(button);
    const done = busyButton(button, words.busy);
    try {
      const { method, path, body } = requestFor(entry, args);
      let answer;
      try {
        try {
          answer = await request(method, path, body);
        } catch (error) {
          // The lock fell between the dialog and the call: ask again, once.
          if (error.id !== 'ADM-106' || !(await askToUnlock(session(), words.unlock))) throw error;
          answer = await request(method, path, body);
        }
      } catch (error) {
        if (gotNoAnswer(error) && entry.changes) {
          notice(LOST_ANSWER, { kind: 'warn', action: { label: 'Open Tasks', run: () => openTasks() } });
        } else {
          problem(error, where);
        }
        return { ok: false, error };
      }
      if (answer && typeof answer.task === 'number') {
        learnTask(answer.task);
        notice(taskStarted(words, answer.task), { action: { label: 'Open Tasks', run: () => openTasks() } });
        return { ok: true, task: answer.task, result: answer };
      }
      if (answer?.done) notice(changeDone(words, answer), { kind: 'ok' });
      return { ok: true, result: answer };
    } finally {
      if (button) running.delete(button);
      done();
    }
  };
}
