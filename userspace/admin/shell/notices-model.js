// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The toasts of design 9.3 as plain values: at most three at once, the
// newest last; one without a button goes after 8 s, one with a button, or
// kept, stays until it is closed.

/** The most notices shown together. */
export const MAX_NOTICES = 3;

/** How long a notice with no button stays. */
export const GONE_MS = 8000;

const KINDS = ['info', 'ok', 'warn'];

/** The notices of one shell. */
export function makeNotices() {
  let next = 1;
  let shown = [];
  return {
    /**
     * Adds a notice. `options` is `{kind, action: {label, run}, keep}`. The
     * same words again, of the same kind, are the notice already shown
     * (`{notice, fresh: false}`), so that a loop cannot fill the screen.
     * Gives `{notice, fresh, dropped}`, where `dropped` holds the notices
     * pushed out to keep to three: the oldest that goes by itself first.
     */
    add(words, { kind = 'info', action = null, keep = false } = {}) {
      const same = shown.find((one) => one.words === words && one.kind === kind);
      if (same) return { notice: same, fresh: false, dropped: [] };
      const notice = {
        id: next, kind: KINDS.includes(kind) ? kind : 'info', words, action, keep: Boolean(keep || action),
      };
      next += 1;
      shown = [...shown, notice];
      const dropped = [];
      while (shown.length > MAX_NOTICES) {
        const gone = shown.find((one) => !one.keep) ?? shown[0];
        dropped.push(gone);
        shown = shown.filter((one) => one !== gone);
      }
      return { notice, fresh: true, dropped };
    },

    /** Takes the notice `id` away. Gives it, or null when it is not there. */
    close(id) {
      const gone = shown.find((one) => one.id === id) ?? null;
      shown = shown.filter((one) => one.id !== id);
      return gone;
    },

    /** The notices, oldest first. */
    list: () => [...shown],
  };
}

/** How long `notice` stays before it goes by itself: null when it stays until closed. */
export const lifetime = (notice) => (notice.keep ? null : GONE_MS);
