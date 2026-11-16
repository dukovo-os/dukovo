// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What a selection in the text of the report does: the card that draws the text
// is given what the browser says is selected (the page's test cannot, the fake
// page has no selection), and shows Cut these words or Cut these lines beside it
// only for what the system gathered, asks for the cuts as reportd takes them,
// and keeps the button while it is pressed. selection.js is read against a
// range made of plain objects; what a real range answers is the lab's test.

import assert from 'node:assert/strict';
import test from 'node:test';

import { makePreview } from '../pages/report/preview.js';
import { selectedIn } from '../pages/report/selection.js';
import { Element, usePage } from './fake-dom.mjs';
import { crashBase, makeDesk } from './report-fixtures.mjs';
import { settle } from './report-view.mjs';

/**
 * A card drawn for the draft of the php-fpm crash. `selection` is what the
 * test says is selected; `cuts` records what the card asked for.
 */
function rig(context, { cuts = [] } = {}) {
  const page = usePage(context);
  const documentListeners = [];
  page.document.addEventListener = (type, listener) => documentListeners.push([type, listener]);
  Object.defineProperty(Element.prototype, 'style', { get() { return (this.styles ??= {}); }, configurable: true });
  context.after(() => { delete Element.prototype.style; });
  const asked = [];
  const state = { selection: null };
  const desk = makeDesk({ base: crashBase });
  const first = desk.draft({ about: 'crash', happened: 'php-fpm stopped', crash: 'c' });
  const preview = makePreview({ signal: new AbortController().signal }, {
    onCut: async (next) => { asked.push(next); },
    onAnswers: async () => true,
    onUndo: () => {},
    onAgain: () => {},
    readSelection: () => state.selection,
  });
  page.document.body.append(preview.element);
  preview.paint({
    mode: 'ready', drafting: false, first, now: first, cuts, edited: false, answers: { happened: 'php-fpm stopped', expected: '', again: '' }, needsFirst: false,
  });
  const rows = () => preview.element.querySelectorAll('.rl').map((node) => ({ n: Number(node.getAttribute('data-n')), text: node.querySelector('.rl-text').textContent, node }));
  const at = (text) => rows().find((one) => one.text === text);
  const button = preview.element.querySelector('.rp-words');
  return {
    ...page,
    asked,
    button,
    rows,
    at,
    preview,
    /** Says `covered` is selected and lets the document know. */
    select: (covered, where = { top: 40, left: 25 }) => {
      state.selection = covered === null ? null : { covered, ...where };
      documentListeners.filter(([type]) => type === 'selectionchange').forEach(([, listener]) => listener());
    },
  };
}

test('words selected in a gathered line show Cut these words beside the selection, and pressing it asks for those characters of the drafted line', async (context) => {
  const card = rig(context);
  const line = card.at('php: [user] signed in');
  assert.equal(card.button.hidden, true);
  card.select([{ n: line.n, from: 5, to: 11 }]);
  assert.equal(card.button.hidden, false);
  assert.equal(card.button.textContent, 'Cut these words');
  assert.deepEqual([card.button.style.top, card.button.style.left], ['44px', '25px']);
  card.button.click();
  await settle();
  assert.deepEqual(card.asked, [[{ section: 'log', line: 1, from: 5, to: 11 }]]);
  assert.equal(card.button.hidden, true, 'the button goes when it is used');
});

test('whole lines selected show Cut these lines, and a run of them is asked for as a run', async (context) => {
  const card = rig(context);
  const first = card.at('php: pool www reloaded');
  const last = card.at('php: signal 11');
  card.select([{ n: first.n, from: 0, to: first.text.length }, { n: last.n, from: 0, to: last.text.length }]);
  assert.equal(card.button.textContent, 'Cut these lines');
  card.button.click();
  await settle();
  assert.deepEqual(card.asked, [[{ section: 'log', line: 2, through: 3 }]]);
});

test('a line selected whole by one drag is a line, not words', async (context) => {
  const card = rig(context);
  const line = card.at('php: signal 11');
  card.select([{ n: line.n, from: 0, to: line.text.length }]);
  assert.equal(card.button.textContent, 'Cut these lines');
});

test('what the system did not gather is not offered: the head, the answers and the headings', async (context) => {
  const card = rig(context);
  for (const text of ['Dukovo 0.16.0, channel stable', 'php-fpm stopped', '== Crash']) {
    const line = card.at(text);
    card.select([{ n: line.n, from: 0, to: 4 }]);
    assert.equal(card.button.hidden, true, text);
  }
  const typed = card.at('php-fpm stopped');
  const crash = card.at('Signature: 3fa2c4d19e0b7a61');
  card.select([{ n: typed.n, from: 0, to: 4 }, { n: crash.n, from: 0, to: crash.text.length }]);
  assert.equal(card.button.hidden, true, 'a run that takes in what was typed');
});

test('lines of two parts are not one cut', async (context) => {
  const card = rig(context);
  const crash = card.at('Signature: 3fa2c4d19e0b7a61');
  const log = card.at('php: listening on 9000');
  card.select([{ n: crash.n, from: 0, to: crash.text.length }, { n: log.n, from: 0, to: log.text.length }]);
  assert.equal(card.button.hidden, true);
});

test('a selection that ends takes the button away, unless the button has the focus', async (context) => {
  const card = rig(context);
  const line = card.at('php: signal 11');
  card.select([{ n: line.n, from: 5, to: 11 }]);
  assert.equal(card.button.hidden, false);
  card.select(null);
  assert.equal(card.button.hidden, true);
  card.select([{ n: line.n, from: 5, to: 11 }]);
  card.button.focus();
  card.select(null);
  assert.equal(card.button.hidden, false, 'pressing the button may end the selection before its click');
  card.button.click();
  await settle();
  assert.equal(card.asked.length, 1);
});

test('the button is kept inside the box: a selection that ends at the far right has its button in view', (context) => {
  const card = rig(context);
  card.preview.element.querySelector('.rp-box').clientWidth = 300;
  const line = card.at('php: signal 11');
  card.select([{ n: line.n, from: 5, to: 11 }], { top: 40, left: 290 });
  assert.equal(card.button.style.left, '150px');
  card.select([{ n: line.n, from: 5, to: 11 }], { top: 40, left: -5 });
  assert.equal(card.button.style.left, '0px');
});

test('arrow keys move between the lines that can be cut, and Tab reaches one of them', async (context) => {
  const card = rig(context);
  const buttons = () => card.preview.element.querySelectorAll('.rl-cut');
  assert.equal(buttons().filter((one) => one.getAttribute('tabindex') === '0').length, 1, 'one line is a Tab stop');
  assert.equal(buttons()[0].getAttribute('tabindex'), '0');
  const text = card.preview.element.querySelector('pre');
  const down = text.fire('keydown', { key: 'ArrowDown', target: text });
  assert.equal(down.defaultPrevented, true);
  assert.equal(buttons()[1].getAttribute('tabindex'), '0');
  assert.equal(buttons()[0].getAttribute('tabindex'), '-1');
  text.fire('keydown', { key: 'ArrowUp', target: text });
  text.fire('keydown', { key: 'ArrowUp', target: text });
  assert.equal(buttons()[0].getAttribute('tabindex'), '0', 'the first stays the first');
  for (let at = 0; at < 12; at += 1) text.fire('keydown', { key: 'ArrowDown', target: buttons()[0] });
  assert.equal(buttons().at(-1).getAttribute('tabindex'), '0', 'the last stays the last');
  assert.equal(card.document.activeElement, buttons().at(-1), 'a key pressed on a button moves focus to the next');
  assert.equal(text.fire('keydown', { key: 'a', target: text }).defaultPrevented, false);
});

// ---- selection.js ----

/** A row of the text made of plain objects: `{cell, parentElement}` with the text it holds. */
function cellOf(n, text) {
  const cell = { text, textContent: text, parentElement: { getAttribute: () => String(n) } };
  cell.contains = (node) => node?.cell === cell;
  return cell;
}

/** A selection of plain objects over `cells`, from `start` to `end` as `[cell, offset]`, ending at the rectangle `rect`. */
function selection(context, { cells, start, end, rect = { bottom: 90, right: 70 } }) {
  const inCell = (cell, offset) => ({ cell, offset });
  const range = {
    commonAncestorContainer: { cell: cells[0] },
    startContainer: inCell(...start),
    startOffset: start[1],
    endContainer: inCell(...end),
    endOffset: end[1],
    intersectsNode: (cell) => {
      const from = cells.indexOf(start[0]);
      const to = cells.indexOf(end[0]);
      const here = cells.indexOf(cell);
      return here >= from && here <= to;
    },
    getClientRects: () => [rect],
    getBoundingClientRect: () => rect,
  };
  const text = {
    contains: () => true,
    querySelectorAll: () => cells,
  };
  const frame = { getBoundingClientRect: () => ({ top: 10, left: 20 }) };
  const was = { document: globalThis.document, getSelection: globalThis.getSelection };
  globalThis.document = {
    createRange: () => {
      const part = {
        selectNodeContents(cell) { part.cell = cell; },
        setEnd(node, offset) { part.offset = offset; part.cell = node.cell; },
        toString: () => part.cell.text.slice(0, part.offset),
      };
      return part;
    },
  };
  globalThis.getSelection = () => ({ rangeCount: 1, isCollapsed: false, getRangeAt: () => range });
  context.after(() => {
    globalThis.document = was.document;
    if (was.getSelection === undefined) delete globalThis.getSelection;
    else globalThis.getSelection = was.getSelection;
  });
  return { text, frame };
}

test('a selection over one row is the row, the offsets in it, and where it ends from the box\'s corner', (context) => {
  const cells = [cellOf(4, 'php: [user] signed in'), cellOf(5, 'php: pool www reloaded')];
  const { text, frame } = selection(context, { cells, start: [cells[0], 5], end: [cells[0], 11] });
  assert.deepEqual(selectedIn(text, frame), { covered: [{ n: 4, from: 5, to: 11 }], top: 80, left: 50 });
});

test('a selection over several rows is the end of the first, the whole of those between and the start of the last; a row with nothing selected is left out', (context) => {
  const cells = [cellOf(4, 'php: [user] signed in'), cellOf(5, 'php: pool www reloaded'), cellOf(6, 'php: signal 11')];
  const { text, frame } = selection(context, { cells, start: [cells[0], 12], end: [cells[2], 4] });
  assert.deepEqual(selectedIn(text, frame).covered, [{ n: 4, from: 12, to: 21 }, { n: 5, from: 0, to: 22 }, { n: 6, from: 0, to: 4 }]);
  const empty = selection(context, { cells, start: [cells[0], 21], end: [cells[1], 0] });
  assert.equal(selectedIn(empty.text, empty.frame), null, 'nothing of either row');
});

test('no selection, a collapsed one, or one outside the text is none', (context) => {
  const cells = [cellOf(4, 'php: signal 11')];
  const { text, frame } = selection(context, { cells, start: [cells[0], 1], end: [cells[0], 3] });
  globalThis.getSelection = () => ({ rangeCount: 0 });
  assert.equal(selectedIn(text, frame), null);
  globalThis.getSelection = () => ({ rangeCount: 1, isCollapsed: true });
  assert.equal(selectedIn(text, frame), null);
  globalThis.getSelection = () => ({
    rangeCount: 1, isCollapsed: false, getRangeAt: () => ({ commonAncestorContainer: {} }),
  });
  assert.equal(selectedIn({ ...text, contains: () => false }, frame), null);
});
