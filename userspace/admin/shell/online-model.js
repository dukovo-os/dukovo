// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Whether the server answers, as plain values (design 4.2): when it is
// asked again and what the banner says.

import { duration } from '../lib/time.js';

/** The waits before each new look at `GET /api/1/health`, then 60 s from then on. */
export const RETRY_SECONDS = [5, 10, 30, 60];

/** After an action restarts the console, the shell is quiet this long before it says offline. */
export const RESTART_QUIET_MS = 120_000;

/** How long to wait before the look number `attempt`, counting from 0. */
export const retryDelay = (attempt) => RETRY_SECONDS[Math.min(attempt, RETRY_SECONDS.length - 1)];

/** What the offline banner says while the next look is `seconds` away. */
export const offlineWords = (host, seconds) => `${host} is not answering. Trying again in ${duration(seconds)}.`;

/** What it says while a look is out. */
export const tryingWords = (host) => `${host} is not answering. Trying now.`;

/** The banner while an action restarts the console. */
export const restartingWords = 'The console is restarting.';

/** The mark on what a page shows while it is stale: "As of 2:02 PM". */
export const asOfWords = (time, seconds) => `As of ${time.clock(seconds)}`;

/** The tip on a button that changes something while the server is not answering. */
export const DISABLED_TIP = 'The server is not answering';

/**
 * The state of the connection: `online`, `offline` (health is asked again
 * at `retryDelay`) or `restarting` (quiet until the time given). All times
 * are milliseconds from any one clock.
 */
export function makeConnection() {
  let attempt = 0;
  let offline = false;
  let quietUntil = 0;
  return {
    /** A call got no HTTP answer. Gives true when this is news (it was online). */
    lost() {
      const news = !offline;
      offline = true;
      return news;
    },
    /** An action will restart the console: say so, and keep quiet for a while. */
    expectRestart(now) {
      quietUntil = now + RESTART_QUIET_MS;
      offline = true;
      attempt = 0;
    },
    /** The look number `attempt` is due in this many seconds, and the next one is later. */
    nextRetry() {
      const seconds = retryDelay(attempt);
      attempt += 1;
      return seconds;
    },
    answered() {
      offline = false;
      attempt = 0;
      quietUntil = 0;
    },
    /** `'online'`, `'offline'` or `'restarting'` at `now`. */
    state(now) {
      if (!offline) return 'online';
      return now < quietUntil ? 'restarting' : 'offline';
    },
  };
}
