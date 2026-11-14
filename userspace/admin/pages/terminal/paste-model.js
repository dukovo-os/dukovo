// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What pasting does (design 13.10): the text is made into what a terminal
// takes (a line ends in CR), the escapes in it go, and it is bracketed
// when the program asked for that (mode 2004). Several lines for a program
// that did not ask are asked about first: each line may run as a command.

export const BRACKET_START = '\x1b[200~';
export const BRACKET_END = '\x1b[201~';

/** The most text one paste may be, in UTF-8 bytes: 1 MB. */
export const MOST_BYTES = 1 << 20;

const encoder = new TextEncoder();

// Control characters other than tab, line feed and carriage return: ESC among them, so that
// the text cannot end the bracket early or start a sequence of its own.
const CONTROLS = /[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/g;

/** The byte arrays `parts`, one after another, in one. */
function joined(...parts) {
  const all = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let at = 0;
  for (const part of parts) {
    all.set(part, at);
    at += part.length;
  }
  return all;
}

/** How many lines `text` has: a line break at its very end does not start another. */
export function linesOf(text) {
  const lines = text.replace(/(\r\n|\r|\n)$/, '').split(/\r\n|\r|\n/);
  return text === '' ? 0 : lines.length;
}

/**
 * Prepares `text` for a program that has bracketed paste on or off:
 * `{ bytes, lines, ask }`, with `ask` the question to put to the person
 * first or null; or `{ tooBig: true }`; or null for nothing to paste.
 */
export function pasteOf(text, bracketed) {
  if (typeof text !== 'string' || text === '') return null;
  // Each UTF-16 unit is a byte or more, so text this long is too much
  // before it is made into bytes.
  if (text.length > MOST_BYTES) return { tooBig: true };
  const lines = linesOf(text);
  const clean = encoder.encode(text.replace(/\r\n|\n/g, '\r').replace(CONTROLS, ''));
  if (clean.length > MOST_BYTES) return { tooBig: true };
  const bytes = bracketed ? joined(encoder.encode(BRACKET_START), clean, encoder.encode(BRACKET_END)) : clean;
  const ask = !bracketed && lines > 1 ? `Paste ${lines} lines? Each line may run as a command.` : null;
  return { bytes, lines, ask };
}
