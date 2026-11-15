// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The terminal's preferences as the Settings page offers them (design 13.10
// and section 5): their limits, which admind's own are the same as, and the
// words of the keys table.

/** The preferences that are numbers: their words and limits (admind's `[terminal]` table). */
export const NUMBERS = [
  {
    key: 'size', label: 'Text size', unit: 'pixels', min: 12, max: 20, step: 1, hint: 'The size of the letters in a terminal, 12 to 20 pixels. The A\u2212 and A+ buttons change it too.',
  },
  {
    key: 'scrollback', label: 'Lines kept', unit: 'lines', min: 1000, max: 50000, step: 1000, hint: 'How many lines above the screen each pane keeps, 1000 to 50000. A pane that is opened again shows what it kept.',
  },
  {
    key: 'keep-days', label: 'Keep a terminal for', unit: 'days', min: 1, max: 30, step: 1, hint: 'A terminal that no browser has open ends after this many days, 1 to 30. It applies to the terminals you open from now on.',
  },
];

/** The preferences that are on or off. */
export const SWITCHES = [
  { key: 'copy-on-select', label: 'Copy as soon as text is selected', hint: 'Selecting text with the mouse copies it. Ctrl+C copies a selection either way.' },
  { key: 'announce', label: 'Read new output aloud', hint: 'A screen reader reads what the program in the pane in focus writes, at most twice a second and 500 characters at a time.' },
  { key: 'shortcuts', label: 'Shortcut keys', hint: 'The Alt+Shift keys below. With them off only Alt+Shift+Up and Alt+Shift+K stay, for people whose system changes the keyboard layout on Alt+Shift.' },
];

/** What `text` is as the number of preference `key`: `{ok: true, value}`, or `{ok: false, words}` that say what is wrong. */
export function checkNumber(key, text) {
  const spec = NUMBERS.find((one) => one.key === key);
  const value = typeof text === 'string' && /^\s*\d+\s*$/.test(text) ? Number(text) : Number.NaN;
  if (!spec || !Number.isInteger(value) || value < spec.min || value > spec.max) {
    return { ok: false, words: `${spec?.label ?? key} is a whole number from ${spec?.min} to ${spec?.max} ${spec?.unit}.` };
  }
  return { ok: true, value };
}

/** The keys, as `[keys, what]` for the table (design 13.10). */
export const TERMINAL_KEYS = [
  ['Alt+Shift+T', 'New terminal'],
  ['Alt+Shift+W', 'End this pane (it asks while a program runs)'],
  ['Alt+Shift+PageUp, Alt+Shift+PageDown', 'The tab before, the tab after'],
  ['Alt+Shift+arrow keys', 'The pane in that direction'],
  ['Alt+Shift+Up in a top pane', 'Leave the pane for the tabs: the way out of the terminal'],
  ['Alt+Shift+\\, Alt+Shift+-', 'Split right, split down'],
  ['Delete on a terminal tab', 'Close the terminal (it asks while a program runs)'],
  ['Alt+Shift+K', 'Search the console'],
  ['Alt+Shift+R', 'Read the screen'],
  ['Ctrl+C', 'Copy when text is selected, else it goes to the program'],
  ['Ctrl+Shift+V, Shift+Insert', 'Paste'],
  ['Shift+PageUp, Shift+PageDown', 'Scroll the lines above the screen'],
];
