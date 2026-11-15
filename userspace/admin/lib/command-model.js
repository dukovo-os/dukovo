// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Show command buttons share, as plain values (design 6): the key
// a call is cached by, so that one `POST /api/1/show` serves an action and
// its arguments for the page's life.

/** One key for an action and its arguments, whatever the order the arguments were given in. */
export function showKey(action, args = {}) {
  const sorted = Object.fromEntries(Object.entries(args).sort(([a], [b]) => (a < b ? -1 : 1)));
  return JSON.stringify([action, sorted]);
}
