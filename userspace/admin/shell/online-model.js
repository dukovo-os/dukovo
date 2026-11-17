// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Whether the server answers, as plain values: when it is asked again and
// what the banner says, while the server is away, while it restarts and when
// it is back.

import { duration } from '../lib/time.js';

/** The waits before each new look at `GET /api/1/health`, then 60 s from then on. */
export const RETRY_SECONDS = [5, 10, 30, 60];

/** After an action restarts the console, the shell is quiet this long before it says offline. */
export const RESTART_QUIET_MS = 120_000;

/** After one restarts the machine: its firmware, disks and services take minutes, and the tries of an update or a bisect take longer. */
export const MACHINE_QUIET_MS = 600_000;

/**
 * How long a look waits for its answer. A server that restarted, or went
 * away without a word, can leave the browser's connection to it open with
 * nothing at the other end, where a call never ends; the browser gives such
 * a connection up, and a later look comes on a new one.
 */
export const LOOK_MS = 10_000;

/** "Back" stays this long, when the restart was as expected. */
export const BACK_MS = 5000;

/** How long to wait before the look number `attempt`, counting from 0. */
export const retryDelay = (attempt) => RETRY_SECONDS[Math.min(attempt, RETRY_SECONDS.length - 1)];

/** What the offline banner says while the next look is `seconds` away. */
export const offlineWords = (host, seconds) => `${host} is not answering. Trying again in ${duration(seconds)}.`;

/** What it says while a look is out. */
export const tryingWords = (host) => `${host} is not answering. Trying now.`;

/** The banner while an action restarts the console, and when a restart has no words of its own. */
export const restartingWords = 'The console is restarting.';

/** Under the restart words: since when the server has not answered. */
export const waitingWords = (time, seconds) => `Waiting since ${time.clock(seconds)}`;

/** The release a server runs, from `release` in `console.about`'s answer ("Dukovo Server 1.0.0-alpha.2"): its last word, or null when the answer tells none. */
export function runningVersion(about) {
  const words = typeof about?.release === 'string' ? about.release.trim().split(/\s+/) : [];
  return words.at(-1) || null;
}

/**
 * What the banner says when the server answers after a restart. `about` is
 * `console.about`'s answer, null when it could not be read; `version` is the
 * release an update was to bring, null for any other restart. Gives `{words,
 * link}`; `link` is `{label, href}` when the update was taken back, else null.
 */
export function backWords(about, version) {
  const running = runningVersion(about);
  if (!version || !running || running === version) return { words: 'Back', link: null };
  return { words: `Back, on ${running}: the update was taken back.`, link: { label: 'See what happened', href: '/updates#happened' } };
}

/** The mark on what a page shows while it is stale: "As of 2:02 PM". */
export const asOfWords = (time, seconds) => `As of ${time.clock(seconds)}`;

/** The tip on a button that changes something while the server is not answering. */
export const DISABLED_TIP = 'The server is not answering';

/**
 * The state of the connection: `online`, `offline` (health is asked again
 * at `retryDelay`) or `restarting` (quiet until the time given, with the words
 * the banner says meanwhile). All times are milliseconds from any one clock.
 */
export function makeConnection() {
  let attempt = 0;
  let offline = false;
  let quietUntil = 0;
  let saying = restartingWords;
  return {
    /** A call got no HTTP answer. Gives true when this is news (it was online). */
    lost() {
      const news = !offline;
      offline = true;
      return news;
    },
    /** An action will restart the console or the machine: say `words`, and keep quiet for `quietMs`. */
    expectRestart(now, { words = restartingWords, quietMs = RESTART_QUIET_MS } = {}) {
      quietUntil = now + quietMs;
      saying = words;
      offline = true;
      attempt = 0;
    },
    /** What the banner says while the connection is quiet. */
    words: () => saying,
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
