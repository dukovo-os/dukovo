// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The list of log lines (design 5.4, 5.5 and 14), for the Logs page and each
// page that shows lines. It is `role="log"`, with each row a button that
// opens its details; rows folded when asked; older lines added at the top and
// new lines at the bottom while the person is there, and a button "12 new
// lines" when they have scrolled up; a polite count that is not every line.
// Only one row is in the tab order, and the arrow keys move between rows, so
// that a keyboard does not step through a thousand lines. The list holds the
// records; the page's filters, reads and buttons are its own. Nothing here
// runs at import.

import { h, uniqueId } from '../dom.js';
import {
  keyOf, makeAnnouncer, merge, newLinesWords, rowsOf,
} from './log-model.js';
import {
  dayWords, stampOf, timeOf, wallOf,
} from './when-model.js';

/** The most rows a list that follows keeps: the oldest go, and Show older reads them again. */
export const MOST_ROWS = 5000;

const LEVELS = { error: 'e', warning: 'w', info: 'i', debug: 'd' };

/** Who wrote a record, as a row says it. */
const fromWords = (record) => (record.from === 'kernel' ? 'kernel' : record.name);

/** Who wrote a record, as the details say it. */
function sourceWords(record) {
  if (record.from === 'kernel') return 'the kernel';
  return `${record.from === 'service' ? 'service' : 'program'} ${record.name}`;
}

/**
 * The list. `zones()` gives the zones the details show a line's time in,
 * `[{label, name}]`; `actions(record, {close})` gives the elements of the
 * buttons under an opened row; `onOpen(record)` is told when a row opens.
 * Gives `element` (the list and what belongs to it, to put in a page) and
 * `set`, `merge`, `records`, `setFolded`, `setZone`, `loading`, `clear`,
 * `toBottom`, `stopCounting`, `focus` and `reveal`, which each have their
 * own words below.
 */
export function makeLogList({
  label = 'Log lines, newest last', zones = () => [], actions = () => [], onOpen = null,
} = {}) {
  const scroller = h('div', { class: 'loglist', role: 'log', 'aria-label': label, 'aria-live': 'off' });
  const newLines = h('button', { class: 'btn new-lines', type: 'button', hidden: true });
  const polite = h('div', { class: 'sr-only', role: 'status', 'aria-live': 'polite' });
  const element = h('div', { class: 'loglist-box' }, scroller, newLines, polite);

  let records = [];
  let rows = [];
  let folded = false;
  let zone = 'UTC';
  // The cells of the rows drawn, by the row's key, so that a row keeps its place, its focus and its opened details when the list is drawn again.
  let cells = new Map();
  let opened = null;
  let current = null;
  let unseen = 0;

  const announcer = makeAnnouncer((words) => {
    // Cleared first, so that the same count told twice is told twice.
    polite.textContent = '';
    setTimeout(() => { polite.textContent = words; }, 50);
  });

  const atBottom = () => scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 24;

  function showUnseen() {
    newLines.hidden = unseen === 0;
    newLines.textContent = unseen === 0 ? '' : newLinesWords(unseen);
  }

  function toBottom() {
    scroller.scrollTop = scroller.scrollHeight;
    unseen = 0;
    showUnseen();
  }

  scroller.addEventListener('scroll', () => {
    if (unseen > 0 && atBottom()) {
      unseen = 0;
      showUnseen();
    }
  });
  newLines.addEventListener('click', () => {
    toBottom();
    focusRow(cells.get(rows[rows.length - 1]?.key));
  });

  // ---- one row ----

  function setOpen(cell, open) {
    cell.button.setAttribute('aria-expanded', String(open));
    cell.detail.hidden = !open;
    cell.wrap.classList.toggle('on', open);
    if (open) fillDetail(cell);
  }

  function close(cell) {
    setOpen(cell, false);
    if (opened === cell) opened = null;
  }

  function fillDetail(cell) {
    const { record } = cell.row;
    const items = [
      ...zones().map((one) => [one.label === one.name ? `Logged, ${one.label}` : `Logged, ${one.label} (${one.name})`, stampOf(record.time, one.name) ?? record.time]),
      ['Importance', record.audit ? `${record.level}, a security event` : record.level],
      ['From', sourceWords(record)],
      record.process === null ? null : ['Process', String(record.process)],
      record.user === null ? null : ['User', String(record.user)],
      ['Start', record.boot],
      ['Number', String(record.seq)],
    ].filter(Boolean);
    cell.detail.replaceChildren(
      h('dl', {}, ...items.flatMap(([name, words]) => [h('dt', {}, name), h('dd', {}, words)])),
      h('div', { class: 'btns' }, ...actions(record, { close: () => close(cell) })),
    );
  }

  function makeCell(row) {
    const id = uniqueId('lline');
    const time = h('span', { class: 'tm' });
    const dot = h('span', { class: 'lv', 'aria-hidden': 'true' });
    const level = h('span', { class: 'sr-only' });
    const from = h('span', { class: 'from' });
    const text = h('span', { class: 'tx' });
    const button = h('button', {
      class: 'lrow', type: 'button', tabindex: '-1', 'aria-expanded': 'false', 'aria-controls': id,
    }, time, dot, level, from, text);
    const detail = h('div', { class: 'ldet', id, hidden: true });
    const wrap = h('div', { class: 'lline' }, button, detail);
    const cell = {
      row, wrap, button, detail, time, dot, level, from, text,
    };
    button.addEventListener('click', () => {
      setCurrent(cell);
      if (opened && opened !== cell) close(opened);
      const open = button.getAttribute('aria-expanded') !== 'true';
      setOpen(cell, open);
      opened = open ? cell : null;
      if (open) onOpen?.(cell.row.record);
    });
    wrap.addEventListener('keydown', (event) => {
      if (event.key !== 'Escape' || button.getAttribute('aria-expanded') !== 'true') return;
      event.stopPropagation();
      close(cell);
      button.focus();
    });
    return cell;
  }

  /** Makes the words of a cell say its row: the time in the zone shown, the level, the source and the text, and for a folded row how often it came. */
  function syncCell(cell, row) {
    cell.row = row;
    const { record } = row;
    cell.time.textContent = wallOf(timeOf(record.time).ms, zone).time;
    cell.dot.className = `lv ${LEVELS[record.level] ?? ''}`;
    cell.level.textContent = `${record.level} `;
    cell.from.textContent = fromWords(record);
    const repeat = row.count === 1 ? null : h('span', { class: 'fold' }, `repeated ${row.count} times, last at ${wallOf(timeOf(row.last.time).ms, zone).time}`);
    cell.text.replaceChildren(...(repeat ? [repeat, ' ', record.text] : [record.text]));
  }

  // ---- the keys ----

  function setCurrent(cell) {
    if (current && current !== cell) current.button.tabIndex = -1;
    current = cell;
    if (cell) cell.button.tabIndex = 0;
  }

  function focusRow(cell) {
    if (!cell) return;
    setCurrent(cell);
    cell.button.focus();
  }

  scroller.addEventListener('keydown', (event) => {
    const button = event.target.closest?.('.lrow');
    if (!button || event.altKey || event.ctrlKey || event.metaKey) return;
    const order = rows.map((row) => cells.get(row.key)).filter(Boolean);
    const at = order.findIndex((cell) => cell.button === button);
    const moves = {
      ArrowDown: at + 1, ArrowUp: at - 1, PageDown: at + 10, PageUp: at - 10, Home: 0, End: order.length - 1,
    };
    if (!Object.hasOwn(moves, event.key) || at < 0) return;
    event.preventDefault();
    focusRow(order[Math.min(order.length - 1, Math.max(0, moves[event.key]))]);
  });

  // ---- drawing ----

  /** Draws every row again, keeping the cells it has. */
  function render() {
    rows = rowsOf(records, folded);
    const next = new Map();
    const nodes = [];
    let day = null;
    for (const row of rows) {
      const { date } = wallOf(timeOf(row.record.time).ms, zone);
      if (date !== day) {
        day = date;
        nodes.push(h('div', { class: 'lday' }, dayWords(date)));
      }
      const cell = cells.get(row.key) ?? makeCell(row);
      syncCell(cell, row);
      next.set(row.key, cell);
      nodes.push(cell.wrap);
    }
    if (opened && !next.has(opened.row.key)) opened = null;
    cells = next;
    if (!current || !next.has(current.row.key)) {
      current = null;
      setCurrent(next.get(rows[rows.length - 1]?.key) ?? null);
    }
    scroller.replaceChildren(...nodes);
  }

  /** Adds the rows that follow the ones drawn, without drawing those again: lines that come while following are many and small. */
  function renderTail() {
    const before = rows.length;
    rows = rowsOf(records, folded);
    let day = before === 0 ? null : wallOf(timeOf(rows[before - 1].record.time).ms, zone).date;
    // The last row drawn may have grown, if it is folded.
    for (let at = Math.max(0, before - 1); at < rows.length; at += 1) {
      const row = rows[at];
      const known = cells.get(row.key);
      const cell = known ?? makeCell(row);
      syncCell(cell, row);
      if (known) continue;
      cells.set(row.key, cell);
      const { date } = wallOf(timeOf(row.record.time).ms, zone);
      if (date !== day) {
        day = date;
        scroller.append(h('div', { class: 'lday' }, dayWords(date)));
      }
      scroller.append(cell.wrap);
    }
  }

  // ---- what a page does with it ----

  return {
    element,

    /** Replaces the lines with `list` (as `recordsOf` gives them), and shows the bottom. */
    set(list) {
      records = list;
      scroller.removeAttribute('aria-busy');
      render();
      toBottom();
    },

    /**
     * Adds the records of `more` that the list has not got, and gives how many
     * were new. Older ones go above and keep the place the person was at;
     * newer ones go below, and with `live` they scroll into view when the
     * person is at the bottom, and are counted for the button and the
     * polite region when they are not.
     */
    merge(more, { live = false } = {}) {
      const result = merge(records, more);
      if (result.added === 0) return 0;
      const bottom = atBottom();
      const first = records[0];
      const oldBefore = records;
      records = result.records;
      const older = oldBefore.length > 0 && records.indexOf(first) === result.added;
      const newer = oldBefore.length === 0 || records.indexOf(oldBefore[oldBefore.length - 1]) === oldBefore.length - 1;
      const capped = live && records.length > MOST_ROWS;
      if (capped) records = records.slice(records.length - MOST_ROWS);
      if (older) {
        const height = scroller.scrollHeight;
        render();
        scroller.scrollTop += scroller.scrollHeight - height;
      } else if (newer && !capped) {
        renderTail();
      } else {
        render();
      }
      if (live) {
        announcer.add(result.added);
        if (bottom) toBottom();
        else {
          unseen += result.added;
          showUnseen();
        }
      }
      return result.added;
    },

    /** The records the list holds, oldest first. */
    records: () => records,

    /** Folds rows that repeat, or stops. */
    setFolded(on) {
      folded = on;
      render();
    },

    /** Shows the times in the zone named `name`. */
    setZone(name) {
      zone = name;
      render();
    },

    /** The state of a read in progress: rows of grey bars, and `aria-busy`. */
    loading() {
      records = [];
      rows = [];
      cells = new Map();
      opened = null;
      current = null;
      unseen = 0;
      showUnseen();
      scroller.setAttribute('aria-busy', 'true');
      scroller.replaceChildren(...Array.from({ length: 8 }, (_, at) => h('div', { class: 'lskel', 'aria-hidden': 'true' }, h('span', { class: `w${at % 4}` }))));
    },

    /** Empties the list. */
    clear() {
      records = [];
      scroller.removeAttribute('aria-busy');
      render();
      unseen = 0;
      showUnseen();
    },

    toBottom,

    /** Stops the count that tells a screen reader the lines that came: a follow has ended. */
    stopCounting: () => announcer.stop(),

    /** Puts focus on the row that is in the tab order (the newest, until the person moves). */
    focus: () => focusRow(current),

    /** Scrolls the row that holds `record` to the middle of the list, and makes it the one in the tab order. */
    reveal(record) {
      const key = keyOf(record);
      const row = rows.find((one) => one.records.some((inside) => keyOf(inside) === key));
      const cell = row && cells.get(row.key);
      if (!cell) return;
      setCurrent(cell);
      cell.wrap.scrollIntoView({ block: 'center' });
    },
  };
}
