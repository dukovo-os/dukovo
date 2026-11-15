// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the console knows of its tasks (design 8), as plain values: the
// tasks from `GET /api/1/tasks` and `event: task`, the count on the header's
// button and the notice for a task that ended. The drawer's groups and the
// words on a card are in tasks-words.js, read when the drawer first opens.
//
// A task is admind's summary: `{id, action, title, args, who, how, started,
// state, step, steps, words, percent, ended, status, note, changed}`, with
// `state` one of waiting, running, done, failed, stopped. An `event: task`
// holds the same without the title, arguments and who.

const RUNNING_STATES = ['waiting', 'running'];
const LOOK_STATES = ['failed', 'stopped'];

/** Whether `task` waits for a place or runs. */
export const isRunning = (task) => RUNNING_STATES.includes(task.state);

/** Whether `task` ended in a way a person should look at: it failed, or it was stopped before its end. */
export const needsLook = (task) => LOOK_STATES.includes(task.state);

/** What the console knows of its tasks, as the list and the events tell it. `event` says whether an id is known; if not, the task is read whole. */
export function makeTasks() {
  const byId = new Map();
  return {
    /** Replaces everything known with `tasks`, as the list at start and after a return from offline gives. */
    replace(tasks) {
      byId.clear();
      for (const task of tasks) byId.set(task.id, task);
    },

    /** Adds `task`, or lets it replace what was known of that id. */
    put(task) {
      byId.set(task.id, { ...byId.get(task.id), ...task });
    },

    /**
     * Applies one `event: task`. Gives `{known: false}` for an id not
     * known; else `{known: true, task, ended}`, `ended` being true when
     * this event is the one that ended the task.
     */
    event(data) {
      const before = byId.get(data.id);
      if (!before) return { known: false };
      const task = { ...before, ...data };
      byId.set(data.id, task);
      return { known: true, task, ended: isRunning(before) && !isRunning(task) };
    },

    get: (id) => byId.get(id),
    all: () => [...byId.values()],
  };
}

/**
 * The header button's count: the tasks that run, or those that need a look
 * that the person has not opened since they ended (`seen` holds their ids),
 * which take the pill in the warn colour. The label says both.
 */
export function pillOf(tasks, seen) {
  const running = tasks.filter(isRunning).length;
  const unseen = tasks.filter((task) => needsLook(task) && !seen.includes(task.id)).length;
  const runningWords = running === 0 ? 'none running' : `${running} running`;
  if (unseen === 0) {
    return { count: running, warn: false, label: `Tasks, ${runningWords}` };
  }
  const look = `${unseen} ${unseen === 1 ? 'needs' : 'need'} a look`;
  return { count: unseen, warn: true, label: `Tasks, ${look}${running > 0 ? `, ${runningWords}` : ''}` };
}

/** The ids of the tasks that need a look, which opening the drawer marks as seen. */
export function lookIds(tasks) {
  return tasks.filter(needsLook).map((task) => task.id);
}

/** What `task` is about, from its arguments: "nginx", or "blog.example.org, 443"; empty when it has none. */
export function subjectOf(task) {
  const values = Object.values(task.args ?? {}).filter((value) => ['string', 'number'].includes(typeof value) && value !== '');
  return values.slice(0, 2).join(', ');
}

/** The card's title: the action's title, and its subject after a dot. */
export function titleOf(task) {
  const title = task.title ?? task.action;
  const subject = subjectOf(task);
  return subject ? `${title} · ${subject}` : title;
}

/** The notice for a task the person started that has ended: polite, and short. */
export function noticeWords(task) {
  const what = `Task ${task.id}, ${task.title ?? task.action}`;
  if (task.state === 'done') return `${what}, finished.`;
  if (task.state === 'stopped') return `${what}, was stopped.`;
  return `${what}, did not work.`;
}
