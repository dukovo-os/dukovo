// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What a screen reader is told of a terminal's new output (design 13.10):
// the text without its escape sequences, handed over at most every 500 ms
// and 500 characters at a time, with "\u2026and 40 more lines" for what was cut.
// A line drawn again from its start, as a shell's line editor draws the
// prompt and what is typed at each key, and a progress bar after a carriage
// return, is said as it was drawn last, as the screen shows it.

export const EVERY_MS = 500;
export const MOST_CHARS = 500;

// Complete sequences: strings (OSC, DCS, SOS, PM, APC), CSI, and the others (ESC, intermediates, a final
// that does not start a longer one).
const STRINGS = /\x1b[\]PX^_][^\x07\x1b]*(?:\x07|\x1b\\)/g;
const SEQUENCES = /\x1b\[[0-?]*[ -/]*[@-~]|\x1b[ -/]*[0-OQ-WYZ\\`-~]/g;
// A carriage return, and a move of the cursor to the first column (CSI H, f
// or G with no column or column 1): what follows is drawn over the line.
const TO_LINE_START = /\x1b\[\d*(?:;0*1?)?[Hf]|\x1b\[0*1?G/g;
// Controls but the carriage return, which stays to say where a line was drawn again.
const CONTROLS = /[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/g;
// A sequence the output ended in the middle of, kept for the next piece.
const CARRY_MOST = 4096;

/**
 * A line as it was drawn last: of the pieces between its returns, the last
 * that is not blank. A return the cursor goes back to after drawing, as
 * after a progress bar's "50%", leaves the line as it was.
 */
function drawnLast(line) {
  return line.split('\r').map((piece) => piece.trim()).filter(Boolean).at(-1) ?? '';
}

export class Announcer {
  #pending = [];

  #carry = '';

  #last = -Infinity;

  /** Adds `text` the program wrote, escape sequences and all. */
  push(text) {
    let all = this.#carry + text;
    this.#carry = '';
    all = all.replace(STRINGS, '').replace(TO_LINE_START, '\r').replace(SEQUENCES, '');
    const cut = all.indexOf('\x1b');
    if (cut >= 0) {
      const rest = all.slice(cut);
      if (rest.length <= CARRY_MOST) this.#carry = rest;
      all = all.slice(0, cut);
    }
    all = all.replace(/\r+\n/g, '\n').replace(/\t/g, ' ').replace(CONTROLS, '');
    // A return alone is kept even with nothing after it: the next piece draws over the line.
    if (all.trim() !== '' || all.includes('\r')) this.#pending.push(all);
  }

  /** Whether there may be something to say. */
  get waiting() {
    return this.#pending.length > 0;
  }

  /** How long until something may be said, in milliseconds. */
  wait(now) {
    return Math.max(0, this.#last + EVERY_MS - now);
  }

  /** The words to say now, or null when there are none or it is too soon. */
  take(now) {
    if (!this.waiting || this.wait(now) > 0) return null;
    const lines = this.#pending.join('').split('\n').map(drawnLast).filter(Boolean);
    this.#pending = [];
    if (lines.length === 0) return null;
    this.#last = now;
    let said = '';
    let used = 0;
    for (const line of lines) {
      const next = said === '' ? line : `${said}\n${line}`;
      if (next.length > MOST_CHARS) {
        if (said === '') {
          said = line.slice(0, MOST_CHARS);
          used += 1;
        }
        break;
      }
      said = next;
      used += 1;
    }
    const more = lines.length - used;
    return more > 0 ? `${said}\n\u2026and ${more} more ${more === 1 ? 'line' : 'lines'}` : said;
  }
}
