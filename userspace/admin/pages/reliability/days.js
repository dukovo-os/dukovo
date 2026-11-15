// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Reliability page (design 7.3) at /reliability: the health score, the
// problems of each day, the events of each day and the day that is chosen,
// the starts, a failed start, the crash dumps and the alerts, over the 7
// days, 30 days or year that the range says (a year counts by the month).
// Each source is read once, when the page opens, when the range changes,
// when the person asks again and when the console is unlocked or locked;
// each card draws when what it needs has come, so that one that cannot read
// says why and the others still draw. Counting is what the page does in
// place of a stability figure, which the machine does not keep (design 12).

import { fill, h } from '../../dom.js';
import { CHECK_NOTES, readChecks } from '../../lib/checks.js';
import {
  DEFAULT_QUERY, MOST_LINES, readPath, recordsOf, urlOf,
} from '../../lib/log-model.js';
import { makeReads } from '../overview/admin-reads.js';
import { commandBox, makeCard } from '../overview/card.js';
import {
  alertRows, barsOf, BARS, bucketsOf, dumpRows, eventsOf, groupsOf, perWords, problemsOf, problemsWords, RANGES, rangeOf,
  tableOf, timelineOf,
} from './model.js';
import { ADMIN_READS, CHANGES_READ, cappedWords } from './reads-model.js';
import { makeFailed, makeStarts } from './starts.js';

const SVG = 'http://www.w3.org/2000/svg';

/** An SVG element with `attributes`; its children are SVG elements. */
function svg(tag, attributes = {}, ...children) {
  const element = document.createElementNS(SVG, tag);
  for (const [name, value] of Object.entries(attributes)) element.setAttribute(name, value);
  element.append(...children);
  return element;
}

/** What a screen reader says of a dot, which the shape and the colour only repeat. */
const SAID = Object.freeze({ bad: 'Did not work', warn: 'Worth a look', info: '' });

/** What the dots of the table mean. */
const LEGEND = Object.freeze([['bad', 'failed or taken back'], ['warn', 'warning'], ['info', 'change or restart']]);

/** The crash dumps the card lists. */
const DUMPS_SHOWN = 5;

/** The list that a read gave, or null when it gave none: `pick` takes the list out of the answer. */
const listOf = (result, pick = (value) => value) => (result.state === 'ok' && Array.isArray(pick(result.value)) ? pick(result.value) : null);

/** The range buttons of the head, each pressed or not: `pick(id)` is called when one is chosen. */
function rangeControl(current, pick) {
  const buttons = RANGES.map((range) => h('button', {
    type: 'button', class: range.id === current ? 'on' : '', 'aria-pressed': String(range.id === current), 'data-range': range.id,
  }, range.label));
  for (const button of buttons) {
    button.addEventListener('click', () => {
      for (const other of buttons) {
        other.classList.toggle('on', other === button);
        other.setAttribute('aria-pressed', String(other === button));
      }
      pick(button.dataset.range);
    });
  }
  return h('span', { class: 'seg', role: 'group', 'aria-label': 'Range' }, ...buttons);
}

/** The bars of the problems a day, as a picture with its words and the numbers behind it. */
function problemsChart(problems, range) {
  const { bars } = barsOf(problems);
  const picture = svg('svg', {
    class: 'chart rel-bars', viewBox: `0 0 ${BARS.width} ${BARS.height}`, preserveAspectRatio: 'none', role: 'img', 'aria-label': problemsWords(problems, range),
  },
  svg('line', { class: 'grid-line', x1: 0, y1: BARS.height, x2: BARS.width, y2: BARS.height }),
  ...bars.filter((bar) => bar.height > 0).map((bar) => svg('rect', {
    class: 'bar', x: bar.x, y: BARS.height - bar.height, width: bar.width, height: bar.height,
  })));
  const total = problems.reduce((sum, one) => sum + one.total, 0);
  const numbers = h('table', { class: 'tbl' },
    h('caption', { class: 'sr-only' }, `Problems ${perWords(range)}, ${range.words}`),
    h('thead', {}, h('tr', {},
      h('th', { scope: 'col' }, h('span', { class: 'sr-only' }, 'When')),
      ...['Failed starts', 'Crashes', 'Errors in the log', 'In all'].map((words) => h('th', { scope: 'col', class: 'num' }, words)))),
    h('tbody', {}, ...problems.map((one) => h('tr', {},
      h('th', { scope: 'row' }, one.words),
      h('td', { class: 'num' }, String(one.failed)), h('td', { class: 'num' }, String(one.crashes)), h('td', { class: 'num' }, String(one.errors)), h('td', { class: 'num' }, String(one.total))))));
  return [
    h('div', { class: 'stat' }, h('b', {}, String(total)), h('span', {}, `failed starts, crashes and errors in the log over ${range.words}`)),
    picture,
    h('div', { class: 'axis', 'aria-hidden': 'true' }, h('span', {}, problems[0].short), h('span', {}, problems.at(-1).short)),
    h('details', { class: 'rel-numbers' }, h('summary', {}, 'Show the numbers'), h('div', { class: 'tbl-box' }, numbers)),
  ];
}

/** One item of a day's timeline: the board's, a time, a dot on the rail and the words. */
function timelineItem(item, time, byDay) {
  const seconds = item.at / 1000;
  return h('div', { class: 'tl-item' },
    h('time', { datetime: new Date(item.at).toISOString(), title: time.full(seconds) }, byDay ? time.clock(seconds) : time.when(seconds)),
    h('span', { class: 'tl-rail' }, h('i', { class: item.dot }, h('span', { class: 'sr-only' }, SAID[item.dot]))),
    h('div', { class: 'tl-body' },
      h('b', {}, item.title),
      item.sub && h('span', { class: 'muted small' }, item.sub),
      item.href && h('a', { class: 'small', href: item.href }, 'Open the start')));
}

export async function show(view) {
  let range = rangeOf(view.query.get('range')).id;
  // The key of the day (or month) chosen in the table, and what the last read drew.
  let chosen = null;
  let drawn = null;
  let latest = 0;
  const reads = makeReads(view);
  let readingAll = reads.mayReadAll();

  // ---- the head ----

  const exportButton = h('button', { class: 'btn', type: 'button', 'aria-expanded': 'false' }, 'Copy settings to Git…');
  const control = rangeControl(range, (id) => {
    range = id;
    chosen = null;
    history.replaceState(null, '', `${location.pathname}?${new URLSearchParams({ range })}`);
    load();
  });
  view.head({ title: 'Reliability', buttons: [control, exportButton], commands: true });

  // ---- the cards ----

  const exportSlot = h('div', { class: 'rel-export' });
  const health = makeCard('health', 'Health');
  health.more.append(h('a', { href: '/' }, 'How the score works'));
  const problems = makeCard('problems', 'Problems a day');
  const events = makeCard('events', 'Events by day');
  const daySlot = h('div', { class: 'rel-day' });
  const failed = makeFailed(view, reads, () => load());
  const starts = makeStarts(view, reads, () => load());
  const crashes = makeCard('crashes', 'Crashes');
  const alerts = makeCard('alerts', 'Alerts');
  view.main.append(h('div', { class: 'rel' },
    exportSlot,
    h('div', { class: 'grid two' }, health.element, problems.element),
    events.element,
    daySlot,
    failed.element,
    starts.element,
    crashes.element,
    alerts.element));

  exportButton.addEventListener('click', async () => {
    if (exportSlot.firstChild) {
      exportSlot.replaceChildren();
      exportButton.setAttribute('aria-expanded', 'false');
      return;
    }
    exportButton.setAttribute('aria-expanded', 'true');
    const { makeExport } = await import('./export.js');
    if (view.signal.aborted) return;
    const form = makeExport(view, {
      close() {
        exportSlot.replaceChildren();
        exportButton.setAttribute('aria-expanded', 'false');
        exportButton.focus();
      },
    });
    exportSlot.replaceChildren(form.element);
    form.focus();
  });

  // ---- drawing ----

  const retry = () => load();

  function drawHealth(checks) {
    if (checks.state !== 'ok') {
      fill(health.body, h('p', { class: 'muted' }, CHECK_NOTES[checks.state] ?? CHECK_NOTES.error));
      return;
    }
    const { score, band } = checks.assessed;
    fill(health.body, h('div', { class: 'rel-health' },
      h('span', { class: `st ${band.id}` }, `${score}, ${band.words.toLowerCase()}`),
      h('span', { class: 'muted small' }, 'out of 100')));
  }

  /** The table of events: one row a kind and a column a day, and under a phone's width a list a day, newest first. */
  function tableNodes(table) {
    const last = table.head.length - 1;
    const pressed = (bucket) => String(bucket.key === chosen);
    const cellButton = (cell, bucket) => h('button', {
      class: 'rel-cell', type: 'button', 'data-key': bucket.key, 'aria-pressed': pressed(bucket), 'aria-label': `${cell.words}, ${bucket.words}`,
    }, h('span', { class: `ev ${cell.dot}`, 'aria-hidden': 'true' }), cell.count > 1 && h('span', { class: 'rel-count', 'aria-hidden': 'true' }, String(cell.count)));
    const wide = h('div', { class: 'tbl-box rel-wide' }, h('table', { class: 'rel-table' },
      h('caption', { class: 'sr-only' }, `Events ${perWords(rangeOf(range))}, ${rangeOf(range).words}`),
      h('thead', {}, h('tr', {},
        h('th', { scope: 'col' }, h('span', { class: 'sr-only' }, 'What happened')),
        ...table.head.map((bucket, at) => h('th', { scope: 'col', class: at === last ? 'today' : '', title: bucket.words }, bucket.short)))),
      h('tbody', {}, ...table.rows.map((row) => h('tr', {},
        h('th', { scope: 'row' }, row.title),
        ...row.cells.map((cell, at) => h('td', { class: at === last ? 'today' : '' }, cell.count > 0 && cellButton(cell, table.head[at]))))))));
    const narrow = h('ul', { class: 'rel-narrow' }, ...[...table.head.keys()].reverse().map((at) => {
      const bucket = table.head[at];
      const found = table.rows.filter((row) => row.cells[at].count > 0);
      return h('li', {},
        h('b', {}, bucket.words),
        found.length === 0
          ? h('span', { class: 'muted small' }, 'Nothing happened.')
          : h('span', { class: 'rel-narrow-cells' }, ...found.map((row) => h('button', {
            class: 'rel-cell', type: 'button', 'data-key': bucket.key, 'aria-pressed': pressed(bucket), 'aria-label': `${row.title}: ${row.cells[at].words}, ${bucket.words}`,
          }, h('span', { class: `ev ${row.cells[at].dot}`, 'aria-hidden': 'true' }), h('span', { 'aria-hidden': 'true' }, row.cells[at].words)))));
    }));
    return [wide, narrow];
  }

  function chooseDay(key) {
    chosen = chosen === key ? null : key;
    for (const button of view.main.querySelectorAll('.rel-cell')) button.setAttribute('aria-pressed', String(button.dataset.key === chosen));
    drawDay(true);
  }

  /** The card of the day that is chosen, or nothing. */
  function drawDay(focus) {
    daySlot.replaceChildren();
    const at = drawn ? drawn.buckets.findIndex((bucket) => bucket.key === chosen) : -1;
    if (at < 0) return;
    const { buckets, groups } = drawn;
    const bucket = buckets[at];
    const byDay = rangeOf(range).unit === 'day';
    const { items, left } = timelineOf(groups[at]);
    const card = makeCard('day', bucket.words);
    card.heading.tabIndex = -1;
    card.more.append(h('a', { href: urlOf({ ...DEFAULT_QUERY, when: 'custom', since: bucket.since, until: bucket.until }) }, `Logs for this ${rangeOf(range).unit}`));
    fill(card.body,
      left > 0 && h('p', { class: 'small muted' }, `${left} earlier ${left === 1 ? 'event is' : 'events are'} not listed.`),
      items.length === 0
        ? h('p', { class: 'muted' }, `Nothing happened in this ${rangeOf(range).unit}.`)
        : h('div', { class: 'tl' }, ...items.map((item) => timelineItem(item, view.time, byDay))),
      byDay && h('a', { href: `/reliability/changes?day=${bucket.key}` }, 'Changes this day'));
    daySlot.append(card.element);
    if (focus) card.heading.focus();
  }

  /** What was not counted, as the lines under the table. */
  function notesOf(results, capped) {
    const lines = [];
    if (results.starts.state !== 'ok') lines.push(reads.note(results.starts, 'the starts', retry));
    if (results.changes.state !== 'ok') lines.push(reads.note(results.changes, 'the changes', retry));
    const admin = [results.records, results.dumps].find((result) => result.state !== 'ok');
    if (admin) lines.push(reads.note(admin, ADMIN_READS, retry));
    for (const words of capped) lines.push(h('p', { class: 'small muted' }, words));
    return lines;
  }

  /** The table, the bars and the day, from the four reads that count. */
  function drawEvents(buckets, results) {
    const sources = {
      starts: listOf(results.starts),
      records: results.records.state === 'ok' ? recordsOf(results.records.value)?.records ?? null : null,
      changes: listOf(results.changes, (value) => value?.changes),
      dumps: listOf(results.dumps),
    };
    const unread = [];
    if (sources.starts === null) unread.push('start', 'update');
    if (sources.records === null) unread.push('error');
    if (sources.changes === null) unread.push('change');
    if (sources.dumps === null) unread.push('crash');
    const groups = groupsOf(buckets, eventsOf(sources, view.time));
    const table = tableOf(buckets, groups, unread);
    const capped = [];
    if ((sources.records?.length ?? 0) >= MOST_LINES) capped.push(cappedWords(MOST_LINES, 'errors'));
    if ((sources.changes?.length ?? 0) >= CHANGES_READ) capped.push(cappedWords(CHANGES_READ, 'changes'));
    drawn = { buckets, groups };
    fill(events.body,
      ...tableNodes(table),
      h('div', { class: 'btns small muted rel-legend' }, ...LEGEND.map(([kind, words]) => h('span', {}, h('span', { class: `ev ${kind}`, 'aria-hidden': 'true' }), ` ${words}`))),
      h('p', { class: 'small muted' }, 'Choose a day, or a month on a year, to see what happened in order.'),
      ...notesOf(results, capped));
    for (const button of events.body.querySelectorAll('.rel-cell')) button.addEventListener('click', () => chooseDay(button.dataset.key));
    fill(problems.body, ...problemsChart(problemsOf(buckets, groups), rangeOf(range)),
      unread.length > 0 && h('p', { class: 'small muted' }, 'What could not be read is not counted.'));
    drawDay(false);
  }

  function drawCrashes(result) {
    if (result.state !== 'ok') {
      fill(crashes.body, reads.note(result, ADMIN_READS, retry));
      return;
    }
    const rows = dumpRows(Array.isArray(result.value) ? result.value : [], view.time);
    if (rows.length === 0) {
      fill(crashes.body, h('p', { class: 'muted' }, 'No crash dumps are kept.'));
      return;
    }
    fill(crashes.body,
      h('ul', { class: 'rel-dumps' }, ...rows.slice(0, DUMPS_SHOWN).map((row) => h('li', {},
        h('b', {}, row.when, row.size && h('span', { class: 'muted' }, ` · ${row.size}`)),
        row.line && h('span', { class: 'mono small' }, row.line),
        commandBox(row.command, { title: 'To read the dump' })))),
      rows.length > DUMPS_SHOWN && h('p', { class: 'small muted' }, `And ${rows.length - DUMPS_SHOWN} older. dumps list shows them all.`));
  }

  function drawAlerts(result) {
    if (result.state !== 'ok') {
      fill(alerts.body, reads.note(result, ADMIN_READS, retry));
      return;
    }
    const lines = result.value;
    const rows = alertRows(Array.isArray(lines) ? lines : [], view.time);
    if (rows.length === 0) {
      fill(alerts.body, h('p', { class: 'muted' }, 'No alert has been raised or cleared.'));
      return;
    }
    fill(alerts.body, h('div', { class: 'tbl-box' }, h('table', { class: 'tbl stack rel-tbl' },
      h('caption', { class: 'sr-only' }, `The last ${rows.length} changes of the alerts, newest first`),
      h('thead', {}, h('tr', {}, ...['When', 'What', 'How bad', 'Source', 'Alert'].map((words) => h('th', { scope: 'col' }, words)))),
      h('tbody', {}, ...rows.map((row) => h('tr', {},
        h('th', { scope: 'row' }, row.when),
        h('td', { 'data-label': 'What' }, row.state),
        h('td', { 'data-label': 'How bad' }, h('span', { class: `ev ${row.dot}`, 'aria-hidden': 'true' }), ` ${row.severity}`),
        h('td', { 'data-label': 'Source' }, row.source),
        h('td', { 'data-label': 'Alert' }, row.title)))))));
  }

  // ---- reading ----

  /** Reads what the page counts and shows, and draws each card when what it needs has come. Only the last read counts. */
  function load() {
    latest += 1;
    const mine = latest;
    readingAll = reads.mayReadAll();
    const buckets = bucketsOf(rangeOf(range), Date.now(), view.time.zone);
    const since = Math.floor(buckets[0].since / 1000);
    const got = {
      starts: reads.read('/api/1/starts'),
      failed: reads.read('/api/1/starts/failed'),
      changes: reads.read(`/api/1/changes?since=${since}&limit=${CHANGES_READ}`),
      records: reads.read(readPath({ lines: MOST_LINES, priority: 'error', since }, { all: true }), { admin: true }),
      dumps: reads.read('/api/1/dumps', { admin: true }),
      alerts: reads.read('/api/1/alerts/history', { admin: true }),
    };
    /** Draws with the result of `promise`, unless the page was left or another read began. */
    const after = (promise, draw) => promise.then((result) => {
      if (!view.signal.aborted && mine === latest) draw(result);
    }, () => {});
    after(readChecks(view), drawHealth);
    after(got.starts, (result) => starts.draw(result));
    after(got.failed, (result) => failed.draw(result));
    after(got.dumps, drawCrashes);
    after(got.alerts, drawAlerts);
    after(Promise.all([got.starts, got.records, got.changes, got.dumps]).then(([a, b, c, d]) => ({
      starts: a, records: b, changes: c, dumps: d,
    })), (results) => drawEvents(buckets, results));
  }

  // The unlock or the lock changes what an administrator's reads give.
  view.on('unlock', () => {
    reads.forget();
    if (reads.mayReadAll() !== readingAll) load();
  });
  load();
}
