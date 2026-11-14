// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Sessions page's rows, the Settings page's limits and keys table, and
// the navigation badge and palette rows the Terminal gives the shell.

import assert from 'node:assert/strict';
import test from 'node:test';

import { badgeOf, count } from '../pages/terminal/badge.js';
import { itemsOf, providers } from '../pages/terminal/palette.js';
import { asksToEnd, openWords, rowsOf, runningWords } from '../pages/terminal/sessions-model.js';
import {
  checkNumber, NUMBERS, SWITCHES, TERMINAL_KEYS,
} from '../pages/terminal/settings-model.js';
import { pageAction } from '../pages/terminal/term/keys.js';
import { makeTime } from '../lib/time.js';

const pane = (id, extra = {}) => ({
  id, admin: false, cols: 80, rows: 24, busy: false, started: 1, attached: 0, ...extra,
});
const terminal = (id, panes, extra = {}) => ({
  id, name: `t${id}`, made: 1000, used: 2000, from: '10.0.0.4', keep_days: 7, layout: 'one', sizes: [], panes, ...extra,
});

test('a terminal is open in this tab, another, both or none', () => {
  const mine = new Set([2]);
  assert.equal(openWords(terminal(1, [pane(1)]), mine), 'not open');
  assert.equal(openWords(terminal(1, [pane(1, { attached: 1 })]), mine), 'in another tab');
  assert.equal(openWords(terminal(1, [pane(2, { attached: 1 })]), mine), 'in this tab');
  assert.equal(openWords(terminal(1, [pane(2, { attached: 2 })]), mine), 'in this tab and another');
  assert.equal(openWords(terminal(1, [pane(2, { attached: 1 }), pane(3, { attached: 1 })]), mine), 'in this tab and another');
  assert.equal(openWords(terminal(1, [pane(2, { attached: 0 })]), mine), 'in this tab', 'a pane about to be shown counts');
});

test('a program running is told as the panes say, and unknown when admind cannot tell', () => {
  assert.equal(runningWords(terminal(1, [pane(1, { busy: false })])), 'No');
  assert.equal(runningWords(terminal(1, [pane(1, { busy: true })])), 'Yes');
  assert.equal(runningWords(terminal(1, [pane(1, { busy: true }), pane(2, { busy: false })])), '1 of 2 panes');
  assert.equal(runningWords(terminal(1, [pane(1, { busy: null }), pane(2, { busy: null })])), 'Not known');
  assert.equal(runningWords(terminal(1, [pane(1, { busy: null }), pane(2, { busy: true })])), '1 of 2 panes');
  assert.equal(asksToEnd(terminal(1, [pane(1, { busy: true })])), true);
  assert.equal(asksToEnd(terminal(1, [pane(1, { busy: null })])), false, 'ending asks only when a program is known to run');
});

test('the rows come newest use first, in the words of the table', () => {
  const time = makeTime('UTC', () => 4_000_000_000);
  const rows = rowsOf([
    terminal(1, [pane(1)], { used: 100 }), terminal(2, [pane(2, { admin: true, busy: true }), pane(3)], { used: 3_999_040, name: 'root@duke-nas' }),
  ], { time, mine: new Set([2]) });
  assert.deepEqual(rows.map((row) => row.id), [2, 1]);
  assert.deepEqual(
    { name: rows[0].name, panes: rows[0].panes, admin: rows[0].admin, used: rows[0].used, from: rows[0].from, open: rows[0].open, running: rows[0].running, asks: rows[0].asks },
    { name: 'root@duke-nas', panes: 2, admin: true, used: '16 minutes ago', from: '10.0.0.4', open: 'in this tab', running: '1 of 2 panes', asks: true },
  );
  assert.equal(rows[1].admin, false);
  assert.equal(rows[1].open, 'not open');
  assert.match(rows[0].madeFull, /1970/);
  assert.deepEqual(rowsOf([], { time }), []);
});

test('a number preference is checked against admind\'s limits', () => {
  assert.deepEqual(NUMBERS.map((one) => [one.key, one.min, one.max]), [['size', 12, 20], ['scrollback', 1000, 50000], ['keep-days', 1, 30]]);
  assert.deepEqual(checkNumber('size', '16'), { ok: true, value: 16 });
  assert.deepEqual(checkNumber('size', ' 12 '), { ok: true, value: 12 });
  assert.deepEqual(checkNumber('scrollback', '50000'), { ok: true, value: 50000 });
  assert.deepEqual(checkNumber('keep-days', '30'), { ok: true, value: 30 });
  for (const [key, text] of [['size', '11'], ['size', '21'], ['scrollback', '999'], ['scrollback', '50001'], ['keep-days', '0'], ['keep-days', '31'], ['size', ''], ['size', '1.5'], ['size', '-14'], ['size', 'abc'], ['size', '1e1']]) {
    const answer = checkNumber(key, text);
    assert.equal(answer.ok, false, `${key} ${text}`);
    assert.match(answer.words, /whole number from/);
  }
  assert.equal(checkNumber('nothing', '1').ok, false);
  assert.deepEqual(SWITCHES.map((one) => one.key), ['copy-on-select', 'announce', 'shortcuts']);
});

test('the keys table agrees with what the keys do', () => {
  const table = new Map(TERMINAL_KEYS);
  assert.equal(table.size, TERMINAL_KEYS.length);
  const alt = (code) => ({ key: code, code, altKey: true, shiftKey: true, ctrlKey: false, metaKey: false });
  const expected = {
    KeyT: 'new-terminal', KeyW: 'end-pane', PageUp: 'previous-tab', PageDown: 'next-tab', ArrowLeft: 'pane-left', ArrowRight: 'pane-right', ArrowUp: 'pane-up', ArrowDown: 'pane-down',
    Backslash: 'split-right', Minus: 'split-down', KeyK: 'palette', KeyR: 'read-screen',
  };
  for (const [code, action] of Object.entries(expected)) assert.equal(pageAction(alt(code)), action, code);
  assert.ok(table.has('Alt+Shift+T') && table.has('Alt+Shift+W') && table.has('Alt+Shift+K') && table.has('Alt+Shift+R'));
  assert.ok([...table.keys()].some((keys) => keys.includes('Alt+Shift+Up in a top pane')));
  assert.ok([...table.keys()].some((keys) => keys.includes('Ctrl+Shift+V') && keys.includes('Shift+Insert')));
  assert.equal(pageAction({ key: 'V', code: 'KeyV', ctrlKey: true, shiftKey: true, altKey: false, metaKey: false }), 'paste');
  assert.equal(pageAction({ key: 'Insert', code: 'Insert', ctrlKey: false, shiftKey: true, altKey: false, metaKey: false }), 'paste');
});

test('the badge counts the terminals and says nothing when there are none', async () => {
  assert.deepEqual(badgeOf({ terminals: [{}, {}, {}] }), { count: 3, label: 'running' });
  assert.equal(badgeOf({ terminals: [] }), null);
  assert.equal(badgeOf(null), null);
  assert.equal(badgeOf({ terminals: 'no' }), null);
  const seen = [];
  const cx = { cache: (key, ms, make) => { seen.push([key, ms]); return make(); }, api: { get: async (path) => ({ terminals: [{ path }] }) } };
  assert.deepEqual(await count(cx), { count: 1, label: 'running' });
  assert.deepEqual(seen, [['/api/1/terminals', 5000]], 'the answer is shared with the other parts that read it');
});

test('the palette finds new terminals and each session by name', async () => {
  const answer = { terminals: [terminal(4, [pane(1), pane(2)], { name: 'logs' }), terminal(9, [pane(3)], { name: 'build' })] };
  const person = itemsOf(answer, { admin: false });
  assert.deepEqual(person.map((one) => [one.key, one.title, one.href]), [
    ['new', 'New terminal', '/terminal?new'], ['session-4', 'logs', '/terminal?open=4'], ['session-9', 'build', '/terminal?open=9'],
  ]);
  assert.equal(person[1].detail, 'Session, 2 panes');
  assert.equal(person[2].detail, 'Session, 1 pane');
  const admin = itemsOf(answer, { admin: true });
  assert.deepEqual(admin.slice(0, 2).map((one) => [one.title, one.href]), [['New terminal', '/terminal?new'], ['New administrator terminal', '/terminal?new=admin']]);
  assert.deepEqual(itemsOf(null, null).map((one) => one.key), ['new']);
  assert.equal(new Set(admin.map((one) => one.key)).size, admin.length, 'every row has a key of its own');
  assert.equal(providers[0].group, 'Terminal');
  const cx = { session: { admin: true }, cache: (key, ms, make) => make(), api: { get: async () => answer } };
  assert.equal((await providers[0].items(cx)).length, 4);
});
