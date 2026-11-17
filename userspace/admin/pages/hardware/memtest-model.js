// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the memory card says of the test at the next restart, as plain values:
// how long it takes, how long a restart for it may stay quiet, and what the
// next start does. The lines about a test that is asked, or was run, are in
// memtest-lines.js, which the card reads when it draws them.

import { upWords } from './model.js';

const GIB = 2 ** 30;
/** The test stops after an hour, whatever is left of the memory. */
const LONGEST = 60;
/** How long a restart for the test may stay quiet when nothing is known of the memory: the most there is. */
const QUIET_MOST = 70;

export const MEMTEST_RESTART_WHY = 'To test the memory before the system starts.';

/** How many minutes the test takes for `bytes` of memory: about one for each 4 GiB, at most an hour; null when the memory is not known. */
export const testMinutes = (bytes) => (bytes > 0 ? Math.min(LONGEST, Math.ceil(bytes / (4 * GIB))) : null);

/** How long a restart for the test may stay quiet before the console says it has not answered, in minutes: ten and one more for each 4 GiB, at most 70. */
export const quietMinutes = (bytes) => Math.min(QUIET_MOST, 10 + (testMinutes(bytes) ?? LONGEST));

/** " (about 4 minutes here)", after the words for the test's time; empty when the memory is not known. */
export function hereWords(bytes) {
  const minutes = testMinutes(bytes);
  if (minutes === null) return '';
  return minutes >= LONGEST ? ' (up to an hour here)' : ` (about ${upWords(minutes * 60)} here)`;
}

/** What the card says of the test at the next restart; `bytes` is the memory's. */
export const nextWords = (bytes) => `The next start tests the free memory before the system starts, about a minute for each 4 GiB${hereWords(bytes)}, takes any bad part out of use, then starts as usual. The server is offline meanwhile, and nobody needs to be at it.`;
