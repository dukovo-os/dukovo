// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What a restart says, as plain values: the words the banner holds while
// the server is away, how long it stays quiet before it says the server is
// not answering, and the words of a restart or shut down that is waiting
// for its time. The dialogs that ask for a restart and the pages that show
// a pending one use these, so that a restart is said the same way wherever
// it begins.

import { MACHINE_QUIET_MS, restartingWords, RESTART_QUIET_MS } from '../shell/online-model.js';

/**
 * The banner's words for a restart of `kind`: `console` (the console's own
 * service), `machine`, `memtest` (the machine tests its memory before it
 * starts, which can take up to an hour), `update` (into `version`, which has
 * three tries to prove itself) or `bisect` (try `try` of `of`). `host` is the
 * machine's name. A kind this does not know says what a machine's restart
 * does.
 */
export function restartWords({
  kind, host, version, try: attempt, of,
}) {
  const name = host ?? 'The machine';
  switch (kind) {
    case 'console':
      return restartingWords;
    case 'memtest':
      return `${name} is restarting and tests its memory first, which can take up to an hour. This page comes back by itself when it answers.`;
    case 'update':
      return `${name} is restarting into ${version ?? 'the new version'}. It has three tries to prove itself; the console answers again in a few minutes.`;
    case 'bisect': {
      const which = attempt && of ? `try ${attempt} of ${of} of changes bisect` : 'a try of changes bisect';
      return `${name} is restarting for ${which}. It restarts again by itself after the try.`;
    }
    default:
      return `${name} is restarting. This page comes back by itself when it answers.`;
  }
}

/**
 * How long the shell stays quiet after a restart of `kind`: the console comes
 * back in a moment, a machine takes minutes. A restart that knows better
 * says its `minutes`, as one that tests the memory first does.
 */
export function quietMsOf(kind, minutes = null) {
  if (minutes > 0) return minutes * 60_000;
  return kind === 'console' ? RESTART_QUIET_MS : MACHINE_QUIET_MS;
}

/** A restart that waits for a minute or less says how many seconds are left; one that waits longer says its time. */
const COUNTED_SECONDS = 60;

const seconds = (count) => `${count} ${count === 1 ? 'second' : 'seconds'}`;

/**
 * The words of the restart or shut down that is waiting, from `shutdown` in
 * `power.status`'s answer (`{end, in_seconds, at_ms}`, or null for none), with
 * `time` for the clock: "Restarts in 52 seconds", "Shuts down at 3:00 AM".
 * Gives null when nothing is waiting.
 */
export function pendingWords(shutdown, time) {
  if (!shutdown) return null;
  const { end, in_seconds: left, at_ms: at } = shutdown;
  const what = end === 'reboot' ? 'Restarts' : 'Shuts down';
  if (typeof left === 'number' && left <= 0) return `${what} now`;
  if (typeof left === 'number' && left <= COUNTED_SECONDS) return `${what} in ${seconds(Math.ceil(left))}`;
  if (typeof at === 'number') return `${what} at ${time.clock(at / 1000)}`;
  return what;
}
