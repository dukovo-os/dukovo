// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The person's preferences in the shell (design 5): read after sign-in,
// changed by a post that is applied at once and undone when the server
// refuses it. theme.js applies the colours, size and motion; `display`
// stands for it, so that a test can give a stand-in.

import { changeFor, displayOf, withTerminalDefaults } from './prefs-model.js';

/**
 * `get` and `post` are api.js's; `display` is theme.js's `dukovoDisplay`
 * (`apply` and `keep`), or null where there is none.
 */
export function createPrefs({ get, post, display }) {
  let value = null;
  const listeners = new Set();
  const show = (prefs) => {
    const shown = displayOf(prefs);
    display?.apply(shown);
    return shown;
  };
  const tell = () => listeners.forEach((listener) => listener(value));

  return {
    /** The preferences, or null until `load` has answered. */
    get value() {
      return value;
    },

    /** Reads them from the server; the server's are the truth, and the copy this browser keeps follows. */
    async load() {
      value = withTerminalDefaults(await get('/api/1/account/preferences'));
      display?.keep(show(value));
      tell();
      return value;
    },

    /**
     * Changes `wanted` (some keys of the preferences): the colours apply at
     * once, and when the server refuses, they go back and the error is
     * thrown. Gives the preferences the server answers.
     */
    async set(wanted) {
      const change = changeFor(value, wanted);
      if (Object.keys(change).length === 0) return value;
      const before = value;
      value = withTerminalDefaults({ ...value, ...change });
      show(value);
      tell();
      try {
        const answer = await post('/api/1/account/preferences', change);
        value = withTerminalDefaults({ ...value, ...answer });
        display?.keep(show(value));
      } catch (error) {
        value = before;
        show(value);
        throw error;
      } finally {
        tell();
      }
      return value;
    },

    /** Shows `wanted` colours, size and motion without saving them; `null` shows the saved ones again. */
    preview(wanted) {
      show(wanted === null ? value : { ...displayOf(value), ...wanted });
    },

    /** Calls `listener` with the preferences whenever they change; the returned function stops it. */
    onChange(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}
