// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What this browser keeps for the console in localStorage: a hidden notice,
// the tasks a person has looked at, the palette's recent things. It is a
// convenience, never the truth: every read and write is in try/catch, since
// a private window or blocked site data make the browser throw, and a page
// that cannot keep something draws as if it kept nothing.

const PREFIX = 'dukovo-admin.';

/** The value kept under `name`, or `fallback` when there is none, it is damaged or the browser refuses. */
export function recall(name, fallback = null) {
  try {
    const text = globalThis.localStorage.getItem(PREFIX + name);
    return text === null ? fallback : JSON.parse(text);
  } catch {
    return fallback;
  }
}

/** Keeps `value` under `name`; false when the browser would not. */
export function keep(name, value) {
  try {
    globalThis.localStorage.setItem(PREFIX + name, JSON.stringify(value));
    return true;
  } catch {
    return false;
  }
}

/** Forgets what is kept under `name`. */
export function forget(name) {
  try {
    globalThis.localStorage.removeItem(PREFIX + name);
  } catch {
    // Nothing was kept that could be seen, so nothing is left to forget.
  }
}

/** A short fingerprint of `text`, to tell a notice's words from the same notice with other words. */
export function fingerprint(text) {
  // FNV-1a over the UTF-16 units: not a secret, only a name for a text.
  let hash = 0x811c9dc5;
  for (let at = 0; at < text.length; at += 1) {
    hash = Math.imul(hash ^ text.charCodeAt(at), 0x01000193) >>> 0;
  }
  return hash.toString(16).padStart(8, '0');
}
