// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The mouse as a program that asked for it reads it: xterm's modes 1000
// (buttons and the wheel) and 1002 (and motion while a button is held),
// in the X10 encoding, or in the SGR one when the program also set 1006.
// The screen holds the modes (`mouseTracking` is 0, 1000 or 1002, and
// `mouseSgr`); this file turns one event into the bytes to send.
//
// An event is `{ action, button, direction, x, y, altKey, ctrlKey, shiftKey }`:
//
//   action     'press', 'release', 'move' or 'wheel'
//   button     0 left, 1 middle, 2 right, as a DOM event numbers them; for
//              'move', the button held
//   direction  for 'wheel': 'up', 'down', 'left' or 'right'
//   x, y       the cell, counted from 0 at the top left
//
// A held Shift is the page's: the person is selecting text, and nothing
// is sent (design 13.10), so Shift is never part of a report.

// The bits of the first number that say what else was held or done.
const ALT = 8;
const CTRL = 16;
const MOTION = 32;
const WHEEL = { up: 64, down: 65, left: 66, right: 67 };
// What X10 reports for any release: which button let go is not said.
const X10_RELEASE = 3;
// A coordinate is sent as one byte, 32 and the 1-based cell, so the last
// cell the X10 form can name is 223.
const X10_LAST = 255 - 33;

/** The bytes to send for `event` in the screen's `modes`, or null when nothing is. */
export function mouseBytes(event, modes = {}) {
  const tracking = modes.mouseTracking;
  if ((tracking !== 1000 && tracking !== 1002) || event.shiftKey) return null;
  const { x, y, action, button } = event;
  if (!Number.isInteger(x) || !Number.isInteger(y) || x < 0 || y < 0) return null;
  let code;
  let release = false;
  if (action === 'wheel') {
    code = Object.hasOwn(WHEEL, event.direction) ? WHEEL[event.direction] : undefined;
  } else if (button === 0 || button === 1 || button === 2) {
    if (action === 'press') code = button;
    else if (action === 'release') [code, release] = [modes.mouseSgr ? button : X10_RELEASE, true];
    else if (action === 'move' && tracking === 1002) code = button + MOTION;
  }
  if (code === undefined) return null;
  if (event.altKey) code += ALT;
  if (event.ctrlKey) code += CTRL;
  if (modes.mouseSgr) {
    const text = `\x1b[<${code};${x + 1};${y + 1}${release ? 'm' : 'M'}`;
    return Uint8Array.from(text, (character) => character.charCodeAt(0));
  }
  if (x > X10_LAST || y > X10_LAST) return null;
  return Uint8Array.of(0x1b, 0x5b, 0x4d, 32 + code, 33 + x, 33 + y);
}

/**
 * Remembers where the last motion was reported, so that motion inside a
 * cell sends nothing, nor does the first move inside the cell of the
 * press: a pointer moves many times in the width of one. Anything else is
 * reported as it is.
 */
export class MouseReporter {
  #last = '';

  /** Like `mouseBytes`, but null for a move to the cell the last one reported. */
  report(event, modes) {
    const bytes = mouseBytes(event, modes);
    if (bytes === null) return null;
    const cell = `${event.x},${event.y},${event.button}`;
    if (event.action === 'move' && cell === this.#last) return null;
    // A press is where the drag starts, and the first move inside its cell is nothing new.
    this.#last = event.action === 'press' || event.action === 'move' ? cell : '';
    return bytes;
  }
}
