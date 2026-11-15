// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The person's preferences as plain values: the choices the account menu
// and the pages offer, what a change to post holds, and the defaults of the
// keys an older server leaves out. The colours, sizes and motion
// themselves are applied by theme.js.

/** The themes, in the order the account menu lists them. */
export const THEME_CHOICES = [
  { value: 'dark', label: 'Dark' },
  { value: 'light', label: 'Light' },
  { value: 'high-contrast', label: 'High contrast' },
  { value: 'device', label: 'Same as this device' },
];

/** The sizes of the text, in percent. */
export const TEXT_CHOICES = [100, 112, 125, 150].map((value) => ({ value, label: `${value}%` }));

/** The ways things may move. */
export const MOTION_CHOICES = [
  { value: 'full', label: 'Full' },
  { value: 'less', label: 'Less' },
];

// What the terminal's keys are when the server says nothing (section 5 of
// the design): the server's own defaults are the same.
const TERMINAL_DEFAULTS = {
  size: 14,
  scrollback: 10000,
  'copy-on-select': false,
  'keep-days': 7,
  announce: false,
  shortcuts: true,
};

/** The colours, size and motion of `prefs`: the part theme.js keeps for the first paint. */
export function displayOf({ theme, text, motion }) {
  return { theme, text, motion };
}

/**
 * What to post to make `wanted` the preferences of a person who has
 * `prefs`: only the keys that differ, so that a change of one choice
 * writes one key.
 */
export function changeFor(prefs, wanted) {
  const change = {};
  for (const [key, value] of Object.entries(wanted)) {
    if (JSON.stringify(prefs[key]) !== JSON.stringify(value)) change[key] = value;
  }
  return change;
}

/** `prefs` with `commands` and the terminal's keys, which a server older than the shell does not send. */
export function withTerminalDefaults(prefs) {
  return {
    ...prefs,
    commands: prefs.commands ?? 'hidden',
    terminal: { ...TERMINAL_DEFAULTS, ...prefs.terminal },
  };
}
