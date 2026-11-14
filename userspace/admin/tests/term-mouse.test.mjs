// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { MouseReporter, mouseBytes } from '../pages/terminal/term/mouse.js';
import { CSI, feed, make } from './term-screen-helpers.mjs';

const X10 = { mouseTracking: 1000, mouseSgr: false };
const X10_DRAG = { mouseTracking: 1002, mouseSgr: false };
const SGR = { mouseTracking: 1000, mouseSgr: true };
const SGR_DRAG = { mouseTracking: 1002, mouseSgr: true };

// A report as the codes it holds, or null.
const codes = (bytes) => (bytes === null ? null : [...bytes]);
// A report as text, for the SGR form.
const spelled = (bytes) => (bytes === null ? null : String.fromCharCode(...bytes));

test('X10: a press is ESC [ M and three bytes', () => {
  assert.deepEqual(codes(mouseBytes({ action: 'press', button: 0, x: 0, y: 0 }, X10)), [0x1b, 0x5b, 0x4d, 32, 33, 33]);
  assert.deepEqual(codes(mouseBytes({ action: 'press', button: 1, x: 9, y: 4 }, X10)), [0x1b, 0x5b, 0x4d, 33, 42, 37]);
  assert.deepEqual(codes(mouseBytes({ action: 'press', button: 2, x: 79, y: 23 }, X10)), [0x1b, 0x5b, 0x4d, 34, 112, 56]);
});

test('X10: a release is button 3 whichever button it was', () => {
  for (const button of [0, 1, 2]) {
    assert.deepEqual(codes(mouseBytes({ action: 'release', button, x: 2, y: 1 }, X10)), [0x1b, 0x5b, 0x4d, 35, 35, 34]);
  }
});

test('X10: the wheel is 64 to 67', () => {
  const directions = { up: 96, down: 97, left: 98, right: 99 };
  for (const [direction, code] of Object.entries(directions)) {
    assert.deepEqual(codes(mouseBytes({ action: 'wheel', direction, x: 0, y: 0 }, X10)), [0x1b, 0x5b, 0x4d, code, 33, 33]);
  }
});

test('X10: Alt adds 8 and Ctrl adds 16', () => {
  assert.equal(mouseBytes({ action: 'press', button: 0, x: 0, y: 0, altKey: true }, X10)[3], 32 + 8);
  assert.equal(mouseBytes({ action: 'press', button: 0, x: 0, y: 0, ctrlKey: true }, X10)[3], 32 + 16);
  assert.equal(mouseBytes({ action: 'press', button: 2, x: 0, y: 0, altKey: true, ctrlKey: true }, X10)[3], 32 + 2 + 8 + 16);
  assert.equal(mouseBytes({ action: 'release', button: 0, x: 0, y: 0, ctrlKey: true }, X10)[3], 32 + 3 + 16);
});

test('X10: the last cell it can name is 223, past it nothing is sent', () => {
  assert.deepEqual(codes(mouseBytes({ action: 'press', button: 0, x: 222, y: 222 }, X10)).slice(4), [255, 255]);
  assert.equal(mouseBytes({ action: 'press', button: 0, x: 223, y: 0 }, X10), null);
  assert.equal(mouseBytes({ action: 'press', button: 0, x: 0, y: 223 }, X10), null);
});

test('SGR: a press is M and a release is m with the real button', () => {
  assert.equal(spelled(mouseBytes({ action: 'press', button: 0, x: 0, y: 0 }, SGR)), '\x1b[<0;1;1M');
  assert.equal(spelled(mouseBytes({ action: 'press', button: 2, x: 99, y: 29 }, SGR)), '\x1b[<2;100;30M');
  assert.equal(spelled(mouseBytes({ action: 'release', button: 0, x: 4, y: 2 }, SGR)), '\x1b[<0;5;3m');
  assert.equal(spelled(mouseBytes({ action: 'release', button: 1, x: 4, y: 2 }, SGR)), '\x1b[<1;5;3m');
  assert.equal(spelled(mouseBytes({ action: 'release', button: 2, x: 4, y: 2 }, SGR)), '\x1b[<2;5;3m');
});

test('SGR: the wheel, the modifiers and cells past 223', () => {
  assert.equal(spelled(mouseBytes({ action: 'wheel', direction: 'up', x: 1, y: 1 }, SGR)), '\x1b[<64;2;2M');
  assert.equal(spelled(mouseBytes({ action: 'wheel', direction: 'down', x: 1, y: 1 }, SGR)), '\x1b[<65;2;2M');
  assert.equal(spelled(mouseBytes({ action: 'wheel', direction: 'left', x: 1, y: 1 }, SGR)), '\x1b[<66;2;2M');
  assert.equal(spelled(mouseBytes({ action: 'wheel', direction: 'right', x: 1, y: 1 }, SGR)), '\x1b[<67;2;2M');
  assert.equal(spelled(mouseBytes({ action: 'press', button: 0, x: 0, y: 0, altKey: true, ctrlKey: true }, SGR)), '\x1b[<24;1;1M');
  assert.equal(spelled(mouseBytes({ action: 'release', button: 0, x: 0, y: 0, ctrlKey: true }, SGR)), '\x1b[<16;1;1m');
  assert.equal(spelled(mouseBytes({ action: 'press', button: 0, x: 499, y: 299 }, SGR)), '\x1b[<0;500;300M');
});

test('1000 reports no motion, 1002 reports it while a button is held', () => {
  assert.equal(mouseBytes({ action: 'move', button: 0, x: 1, y: 1 }, X10), null);
  assert.equal(mouseBytes({ action: 'move', button: 0, x: 1, y: 1 }, SGR), null);
  assert.deepEqual(codes(mouseBytes({ action: 'move', button: 0, x: 1, y: 1 }, X10_DRAG)), [0x1b, 0x5b, 0x4d, 64, 34, 34]);
  assert.deepEqual(codes(mouseBytes({ action: 'move', button: 2, x: 1, y: 1 }, X10_DRAG)), [0x1b, 0x5b, 0x4d, 66, 34, 34]);
  assert.equal(spelled(mouseBytes({ action: 'move', button: 0, x: 1, y: 1 }, SGR_DRAG)), '\x1b[<32;2;2M');
  assert.equal(spelled(mouseBytes({ action: 'move', button: 1, x: 1, y: 1, ctrlKey: true }, SGR_DRAG)), '\x1b[<49;2;2M');
  assert.equal(mouseBytes({ action: 'move', x: 1, y: 1 }, SGR_DRAG), null);
  assert.equal(mouseBytes({ action: 'move', button: -1, x: 1, y: 1 }, X10_DRAG), null);
});

test('with the mouse off, or with Shift held, nothing is sent', () => {
  const press = { action: 'press', button: 0, x: 0, y: 0 };
  assert.equal(mouseBytes(press, { mouseTracking: 0, mouseSgr: true }), null);
  assert.equal(mouseBytes(press, {}), null);
  assert.equal(mouseBytes(press), null);
  assert.equal(mouseBytes({ ...press, shiftKey: true }, SGR), null);
  assert.equal(mouseBytes({ ...press, shiftKey: true }, X10_DRAG), null);
});

test('events that make no report are refused and do not throw', () => {
  for (const event of [
    {},
    { action: 'press', button: 0 },
    { action: 'press', button: 0, x: -1, y: 0 },
    { action: 'press', button: 0, x: 0, y: -1 },
    { action: 'press', button: 0, x: 1.5, y: 0 },
    { action: 'press', button: 0, x: '1', y: 0 },
    { action: 'press', button: 3, x: 0, y: 0 },
    { action: 'press', button: 4, x: 0, y: 0 },
    { action: 'press', x: 0, y: 0 },
    { action: 'click', button: 0, x: 0, y: 0 },
    { action: 'wheel', x: 0, y: 0 },
    { action: 'wheel', direction: 'sideways', x: 0, y: 0 },
    { action: 'wheel', direction: 'constructor', x: 0, y: 0 },
    { action: 'wheel', direction: '__proto__', x: 0, y: 0 },
  ]) {
    assert.equal(mouseBytes(event, SGR), null, JSON.stringify(event));
    assert.equal(mouseBytes(event, X10), null, JSON.stringify(event));
  }
});

test('the modes come from the screen', () => {
  const screen = make(10, 4);
  const press = { action: 'press', button: 0, x: 3, y: 2 };
  assert.equal(mouseBytes(press, screen), null);
  feed(screen, `${CSI}?1000h`);
  assert.deepEqual(codes(mouseBytes(press, screen)), [0x1b, 0x5b, 0x4d, 32, 36, 35]);
  feed(screen, `${CSI}?1006h`);
  assert.equal(spelled(mouseBytes(press, screen)), '\x1b[<0;4;3M');
  assert.equal(mouseBytes({ action: 'move', button: 0, x: 3, y: 2 }, screen), null);
  feed(screen, `${CSI}?1002h`);
  assert.equal(spelled(mouseBytes({ action: 'move', button: 0, x: 3, y: 2 }, screen)), '\x1b[<32;4;3M');
  feed(screen, `${CSI}?1002l`);
  assert.equal(mouseBytes(press, screen), null);
});

test('a drag reports each cell once', () => {
  const reporter = new MouseReporter();
  const at = (action, x, y, button = 0) => reporter.report({ action, button, x, y }, SGR_DRAG);
  assert.equal(spelled(at('press', 2, 2)), '\x1b[<0;3;3M');
  assert.equal(at('move', 2, 2), null);
  assert.equal(spelled(at('move', 3, 2)), '\x1b[<32;4;3M');
  assert.equal(at('move', 3, 2), null);
  assert.equal(at('move', 3, 2), null);
  assert.equal(spelled(at('move', 3, 3)), '\x1b[<32;4;4M');
  assert.equal(spelled(at('move', 3, 2)), '\x1b[<32;4;3M');
  assert.equal(spelled(at('release', 3, 2)), '\x1b[<0;4;3m');
  assert.equal(spelled(at('press', 3, 2, 2)), '\x1b[<2;4;3M');
  assert.equal(at('move', 3, 2, 2), null);
  assert.equal(spelled(at('move', 3, 2, 0)), '\x1b[<32;4;3M');
});

test('a wheel and what is not reported do not disturb the drag', () => {
  const reporter = new MouseReporter();
  assert.ok(reporter.report({ action: 'press', button: 0, x: 1, y: 1 }, SGR_DRAG));
  assert.ok(reporter.report({ action: 'wheel', direction: 'up', x: 1, y: 1 }, SGR_DRAG));
  assert.ok(reporter.report({ action: 'move', button: 0, x: 1, y: 1 }, SGR_DRAG));
  assert.equal(reporter.report({ action: 'move', button: 0, x: 1, y: 1 }, SGR_DRAG), null);
  assert.equal(reporter.report({ action: 'move', button: 0, x: 2, y: 1, shiftKey: true }, SGR_DRAG), null);
  assert.equal(reporter.report({ action: 'move', button: 0, x: 1, y: 1 }, SGR_DRAG), null);
  assert.equal(reporter.report({ action: 'move', button: 0, x: 2, y: 1 }, { mouseTracking: 0 }), null);
  assert.ok(reporter.report({ action: 'move', button: 0, x: 2, y: 1 }, SGR_DRAG));
});
