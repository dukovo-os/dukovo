// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Key events to the bytes a program expects, as the xterm-256color terminfo
// entry the machine ships spells them (ADR 0086), and the keys the page
// keeps for itself.
//
// An event is any object with `key`, `code`, `ctrlKey`, `altKey`,
// `shiftKey` and `metaKey`, a browser's KeyboardEvent or a plain object.
// `key` says what the key makes (so a layout's own letters arrive as they
// are); `code` says which key it is, and is read only where the layout
// must not matter: the page's shortcuts, and Ctrl with a letter on a
// layout that is not Latin.
//
// `keyBytes(event, modes)` gives a Uint8Array to send, or null when the key
// sends nothing: a modifier alone, a dead key or a key mid-composition
// (the page reads the text the input method makes instead), Meta (the
// system's), a key this terminal has no bytes for, and the keys the page
// keeps. `modes` has the screen's `applicationCursor` and
// `applicationKeypad`, and `shortcuts`, the preference that turns the
// page's Alt+Shift keys on (it is on unless it is `false`). The screen
// can be passed as it is.

const encoder = new TextEncoder();

const bytes = (text) => encoder.encode(text);

/**
 * What the page keeps, by `code`, with Alt and Shift held (design 13.10).
 * The second set is what stays when `shortcuts` is off: the way out of the
 * pane and the palette, for people whose system uses Alt+Shift to change
 * the keyboard layout.
 */
const PAGE_KEYS = {
  KeyT: 'new-terminal',
  KeyW: 'end-pane',
  PageUp: 'previous-tab',
  PageDown: 'next-tab',
  ArrowLeft: 'pane-left',
  ArrowRight: 'pane-right',
  ArrowUp: 'pane-up',
  ArrowDown: 'pane-down',
  Backslash: 'split-right',
  Minus: 'split-down',
  KeyK: 'palette',
  KeyR: 'read-screen',
};
const ALWAYS_KEPT = new Set(['pane-up', 'palette']);

/**
 * The page's action for `event`, or null when the key is for the program:
 * 'new-terminal', 'end-pane', 'previous-tab', 'next-tab', 'pane-left',
 * 'pane-right', 'pane-up', 'pane-down', 'split-right', 'split-down',
 * 'palette', 'read-screen' and 'paste'. Pasting (Ctrl+Shift+V and
 * Shift+Insert) is never turned off; the text comes from the paste event,
 * so the key itself sends nothing. Ctrl+C is not here: whether it copies
 * depends on a selection, which the page knows.
 */
export function pageAction(event, shortcuts = true) {
  if (event.metaKey) return null;
  if (event.shiftKey && !event.altKey) {
    if (event.ctrlKey && event.code === 'KeyV') return 'paste';
    if (!event.ctrlKey && event.code === 'Insert') return 'paste';
    return null;
  }
  if (!event.shiftKey || !event.altKey || event.ctrlKey) return null;
  const action = has(PAGE_KEYS, event.code) ? PAGE_KEYS[event.code] : null;
  return action && (shortcuts || ALWAYS_KEPT.has(action)) ? action : null;
}

// Keys that change nothing on their own, or that the input method owns.
const SILENT = new Set([
  'Shift', 'Control', 'Alt', 'Meta', 'AltGraph', 'CapsLock', 'NumLock', 'ScrollLock', 'Fn', 'FnLock',
  'Hyper', 'Super', 'OS', 'Symbol', 'SymbolLock', 'Dead', 'Process', 'Compose', 'Unidentified',
]);

// The keys that end in a letter, after CSI (or SS3 in application cursor
// mode), and the ones that end in a tilde, after CSI and a number.
const LETTER_KEYS = { ArrowUp: 'A', ArrowDown: 'B', ArrowRight: 'C', ArrowLeft: 'D', Home: 'H', End: 'F' };
const FUNCTION_LETTERS = { F1: 'P', F2: 'Q', F3: 'R', F4: 'S' };
const TILDE_KEYS = {
  Insert: 2, Delete: 3, PageUp: 5, PageDown: 6,
  F5: 15, F6: 17, F7: 18, F8: 19, F9: 20, F10: 21, F11: 23, F12: 24,
};

// The keypad in application mode: ESC O and a letter, by what the key makes.
const KEYPAD = {
  0: 'p', 1: 'q', 2: 'r', 3: 's', 4: 't', 5: 'u', 6: 'v', 7: 'w', 8: 'x', 9: 'y',
  '*': 'j', '+': 'k', '-': 'm', '.': 'n', '/': 'o', Enter: 'M',
};

// Ctrl with a character that is not a letter: the C0 control it stands for.
// The digits are for the keys that have no shifted partner on a keyboard
// that has them (Ctrl+2 is NUL, Ctrl+3 is ESC, and so on up to Ctrl+8, DEL).
const CONTROL = {
  '@': 0, ' ': 0, 2: 0, '[': 27, 3: 27, '\\': 28, 4: 28, ']': 29, 5: 29,
  '^': 30, 6: 30, _: 31, '/': 31, '-': 31, 7: 31, '?': 127, 8: 127,
};

const has = (table, key) => Object.hasOwn(table, key);

/** The bytes for `event` in the screen's `modes`, or null (see the top of the file). */
export function keyBytes(event, modes = {}) {
  const { key } = event;
  if (typeof key !== 'string' || event.isComposing || event.metaKey || SILENT.has(key)) return null;
  if (pageAction(event, modes.shortcuts !== false) !== null) return null;
  // The modifier number of the CSI forms: 1 for none, then Shift 1, Alt 2, Ctrl 4.
  const modifier = 1 + (event.shiftKey ? 1 : 0) + (event.altKey ? 2 : 0) + (event.ctrlKey ? 4 : 0);
  if (has(LETTER_KEYS, key)) return letterKey(LETTER_KEYS[key], modifier, modes);
  if (has(FUNCTION_LETTERS, key)) {
    return modifier === 1 ? bytes(`\x1bO${FUNCTION_LETTERS[key]}`) : bytes(`\x1b[1;${modifier}${FUNCTION_LETTERS[key]}`);
  }
  if (has(TILDE_KEYS, key)) {
    return bytes(modifier === 1 ? `\x1b[${TILDE_KEYS[key]}~` : `\x1b[${TILDE_KEYS[key]};${modifier}~`);
  }
  // The keys below send a character, which Alt puts an ESC in front of.
  const sent = single(event, modes);
  if (sent === null) return null;
  return event.altKey && !altGraph(event) ? bytes(`\x1b${sent}`) : bytes(sent);
}

// An arrow, Home or End: `CSI X`, or `SS3 X` when the program asked for
// application cursor keys, and `CSI 1 ; modifier X` with a modifier held,
// which the mode does not change.
function letterKey(final, modifier, modes) {
  if (modifier > 1) return bytes(`\x1b[1;${modifier}${final}`);
  return bytes(modes.applicationCursor ? `\x1bO${final}` : `\x1b[${final}`);
}

// Whether `key` is one character, which is text: a longer name is a key
// this terminal has no bytes for (a media key, F13), and a lone surrogate
// could not be encoded.
function character(key) {
  const point = key.codePointAt(0);
  return key.length === (point > 0xffff ? 2 : 1) && !(point >= 0xd800 && point <= 0xdfff);
}

// Ctrl and Alt together with a character that is no letter can be a
// layout's AltGr (Windows reports it so), which makes a character and is no
// shortcut.
const altGraph = (event) => event.ctrlKey && event.altKey && character(event.key) && !/^[A-Za-z]$/.test(event.key);

// The text a key sends before Alt is looked at, or null.
function single(event, modes) {
  const { key } = event;
  const code = typeof event.code === 'string' ? event.code : '';
  if (key === 'Enter') return modes.applicationKeypad && code === 'NumpadEnter' ? `\x1bO${KEYPAD.Enter}` : '\r';
  if (key === 'Escape') return '\x1b';
  if (key === 'Backspace') return event.ctrlKey ? '\b' : '\x7f';
  if (key === 'Tab') return event.shiftKey ? '\x1b[Z' : '\t';
  if (modes.applicationKeypad && code.startsWith('Numpad') && has(KEYPAD, key)) return `\x1bO${KEYPAD[key]}`;
  if (!character(key)) return null;
  if (event.ctrlKey && !altGraph(event)) return control(event);
  // Where Alt makes another character (Option on a Mac), a program wants the
  // key that was pressed, as ESC then the letter, for Alt+B and Alt+F.
  const letter = key.charCodeAt(0) > 0x7f && event.altKey && /^Key[A-Z]$/.test(code);
  return letter ? (event.shiftKey ? code[3] : code[3].toLowerCase()) : key;
}

// Ctrl and one character: the control code, or the character as it is when
// there is none.
function control(event) {
  const { key } = event;
  const code = typeof event.code === 'string' ? event.code : '';
  if (/^[A-Za-z]$/.test(key)) return String.fromCharCode(key.toLowerCase().charCodeAt(0) - 96);
  if (has(CONTROL, key)) return String.fromCharCode(CONTROL[key]);
  // A layout that is not Latin still has Ctrl+C where the C key is.
  if (key.charCodeAt(0) > 0x7f && /^Key[A-Z]$/.test(code)) return String.fromCharCode(code.charCodeAt(3) - 64);
  return key;
}
