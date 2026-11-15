// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Tasks indicator and drawer (design 8): the model of lib/tasks-model.js
// kept current by `GET /api/1/tasks` and `event: task`, the pill on the
// header's button and a notice when a task the person started ends, which
// takes the place of the one that said it runs. The
// drawer's list is tasks/cards.js, read when the drawer first opens.

import { keep, recall } from '../lib/store.js';
import {
  isRunning, lookIds, makeTasks, noticeWords, pillOf,
} from '../lib/tasks-model.js';

/** How many ids of tasks seen are kept per browser. */
const SEEN_KEPT = 200;

/**
 * `get` and `post` are api.js's; `session()` and `time()` give the current
 * ones; `notice` and `problem` are the page contract's; `commands` is
 * lib/command.js's; `drawer` is the shell's.
 */
export function createTasks({ get, post, session, time, notice, problem, commands, drawer, hasPage }) {
  const model = makeTasks();
  const listeners = new Set();
  let seen = recall('tasks.seen', []);
  let cards = null;
  let looking = false;
  // What closes the notice that a task this tab started runs, by its id.
  const startNotices = new Map();

  const pill = () => pillOf(model.all(), seen);

  // Looking at the list is how a task that needs a look is marked seen: when
  // the list is opened, and for a task that ends while it is open.
  function markSeen() {
    seen = [...new Set([...seen, ...lookIds(model.all())])].slice(-SEEN_KEPT);
    keep('tasks.seen', seen);
  }

  function changed() {
    if (looking) markSeen();
    const now = pill();
    for (const listener of listeners) listener(now);
    cards?.render();
  }

  /** Closes the notice that said the task `id` runs, as it has ended. */
  function closeStarted(id) {
    startNotices.get(id)?.();
    startNotices.delete(id);
  }

  /** Reads one task whole: an event holds no title, arguments or log. */
  async function learn(id) {
    try {
      const task = await get(`/api/1/tasks/${id}`);
      model.put(task);
      if (!isRunning(task)) closeStarted(id);
      changed();
    } catch {
      // The task is gone or the server does not answer; the next list has it.
    }
  }

  const tasks = {
    /** Reads the list: at start, and after each return from offline. */
    async load() {
      const answer = await get('/api/1/tasks');
      model.replace(answer.tasks ?? []);
      changed();
    },

    /** Takes one event of the stream; only `task` is its own. */
    event(name, data) {
      if (name !== 'task') return;
      const found = model.event(data);
      if (!found.known) {
        learn(data.id);
        return;
      }
      if (found.ended) closeStarted(found.task.id);
      if (found.ended && found.task.who === session().name) {
        const failed = found.task.state === 'failed';
        notice(noticeWords(found.task), {
          kind: found.task.state === 'done' ? 'ok' : failed ? 'warn' : 'info',
          action: failed ? { label: 'Show the log', run: () => tasks.open(null, found.task.id) } : null,
        });
      }
      changed();
    },

    /** Reads a task the shell has just been told of, so that the list has it. */
    learn,

    /** Reads a task this tab started; `close` closes the notice that it runs, when it ends. */
    started(id, close) {
      startNotices.set(id, close);
      return learn(id);
    },

    pill,

    /** Calls `listener` with the pill whenever the count or its look changes. */
    onPill(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },

    /** Opens the drawer (or closes it when it is open), with the log of task `logId` shown when given. */
    async open(opener, logId = null) {
      if (!cards) {
        const { makeCards } = await import('./cards.js');
        cards = makeCards({ model, get, post, session, time, notice, problem, commands, hasPage, changed });
      }
      if (logId !== null) cards.showLog(logId);
      const options = {
        title: 'Tasks',
        opener: opener ?? document.activeElement,
        build: (body) => {
          looking = true;
          const gone = cards.build(body);
          changed();
          return () => {
            looking = false;
            gone();
          };
        },
      };
      if (logId === null) drawer.toggle('tasks', options);
      else drawer.open('tasks', options);
    },

    all: () => model.all(),
  };
  return tasks;
}
