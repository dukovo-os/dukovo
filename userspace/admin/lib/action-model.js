// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What running an action decides, as plain values (design 7): the request
// an action and its arguments make, who may run it, and the words the page
// shows on the way. admind's `GET /api/1/actions` lists each action with
// its `method`, `path` (with `{name}` parts), `runs` and `title`.

/** Who runs an action's command when it needs an administrator. */
export const AS_ADMINISTRATOR = 'as-administrator';

/** The action `id` of the list `actions`, or undefined. */
export const findAction = (actions, id) => actions.find((entry) => entry.id === id);

/**
 * The request for `entry` with `args`: `{method, path, body}`. Each
 * `{name}` of the path is an argument, escaped; the others go in the body
 * of a change, or in the query of a read.
 */
export function requestFor(entry, args = {}) {
  const used = new Set();
  const path = entry.path.replace(/\{([^}]+)\}/g, (_, name) => {
    used.add(name);
    return encodeURIComponent(String(args[name] ?? ''));
  });
  const rest = Object.fromEntries(Object.entries(args).filter(([name]) => !used.has(name)));
  if (entry.method === 'GET') {
    const query = new URLSearchParams(Object.entries(rest).map(([name, value]) => [name, String(value)])).toString();
    return { method: 'GET', path: query ? `${path}?${query}` : path, body: undefined };
  }
  return { method: 'POST', path, body: rest };
}

/** Whether `entry` needs an administrator whose session is unlocked. */
export const needsAdministrator = (entry) => entry.runs === AS_ADMINISTRATOR;

/** `words` with its first letter in lower case, for the middle of a sentence. */
export const lowerFirst = (words) => (words ? words[0].toLowerCase() + words.slice(1) : words);

/**
 * What a reader is told in place of a change: admind's ADM-122 says it
 * with the administrators named, but a reader's request is never sent, so
 * these are the words without the names.
 */
export function readerProblem(session) {
  return {
    id: 'ADM-122',
    title: `${session.name}'s account can look but not change`,
    detail: 'This change needs an administrator.',
    fix: 'Ask an administrator to make the change, or to give your account administrator rights.',
    status: 403,
    field: null,
    command: null,
    retryAfter: null,
  };
}

/**
 * The words a page shows for `entry`, with the page's own `words` over the
 * defaults, which come from the action's title:
 * `unlock` ("restart nginx", after "You are about to"), `running`,
 * `done`, `unchanged` and `busy`.
 */
export function wordsFor(entry, words = {}) {
  const title = entry.title ?? entry.id;
  return {
    unlock: words.unlock ?? lowerFirst(title),
    running: words.running ?? title,
    done: words.done ?? `${title}: done.`,
    unchanged: words.unchanged ?? `${title}: nothing changed.`,
    busy: words.busy ?? `${title}…`,
  };
}

/** The notice for a task that started: "Restarting nginx: running as task 42". */
export const taskStarted = (words, id) => `${words.running}: running as task ${id}`;

/** The notice for a change that finished. */
export const changeDone = (words, result) => (result.changed === false ? words.unchanged : words.done);

/** What is said when a request was sent and its answer was lost: it may have run. */
export const LOST_ANSWER = 'The answer did not come back. Look in What changed and Tasks to see if it ran.';

/** Whether `error` is a request that got no HTTP answer at all. */
export const gotNoAnswer = (error) => error.status === 0 && error.id === null;
