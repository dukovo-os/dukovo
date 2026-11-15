// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// When the page tries the socket again (design 13.10): after 1, 2, 5 and
// 10 seconds, then every 30, and the words the footer says meanwhile.

const DELAYS = [1000, 2000, 5000, 10000];
const LONG_DELAY = 30000;

/** The close code admind gives when the session has ended: nothing to try again. */
export const SESSION_ENDED = 4001;

/** How long to wait before try number `attempt` (counting from 1), in milliseconds. */
export function delayFor(attempt) {
  return DELAYS[attempt - 1] ?? LONG_DELAY;
}

/** Whether to try again after the socket closed with `code`. */
export function retries(code) {
  return code !== SESSION_ENDED;
}

/** What the footer says: nothing is wrong while connected, and the try number while it is not. */
export function statusWords({ state, attempt }) {
  if (state === 'open') return 'Connected \u00b7 terminals keep running when you close this page, until the console restarts';
  if (state === 'ended') return 'Disconnected \u00b7 the session ended';
  return attempt > 0 ? `Reconnecting\u2026 try ${attempt}` : 'Connecting\u2026';
}
