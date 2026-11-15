// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The view from a chart (design 6), at `/logs/chart/{chart}?range=R&at=MS`:
// a chart that a moment can be chosen on, and what the machine says about
// the step that holds the moment: what happened around it, which services
// were busiest, what moved most against the hour before, and the log's
// worst lines. The Logs page reads this file when the address says `chart/`,
// so the Logs page itself does not carry it. Each card reads for itself, so a
// card that cannot read says why and the others carry on.

import { h } from '../../dom.js';
import { makeChart } from '../../lib/chart.js';
import {
  CHARTS, formatBytes, formatPercent, modelOf, RANGES, rangeOf, totalMemory,
} from '../../lib/chart-model.js';
import { indexOfTime, pickLabel, timesOf } from '../../lib/chart-pick-model.js';
import { COPIED, copyText, PRESS_CTRL_C } from '../../lib/copy.js';
import { makeLogList } from '../../lib/log-list.js';
import {
  aroundOf, argsOf, DEFAULT_QUERY, lineOf, PAGE_LINES, readPath, recordsOf, urlOf, whoWords,
} from '../../lib/log-model.js';
import { isZone } from '../../lib/time.js';
import { timeOf } from '../../lib/when-model.js';
import { askToUnlock, isUnlocked, onUnlockChange } from '../../unlock.js';
import { failureWords, makeCard } from '../overview/card.js';
import {
  aroundAt, busiest, marksOf, moved, serviceSeries, stepOf, timelineOf,
} from './around-model.js';

/** The most of the journal that the chart marks. */
const JOURNAL_ROWS = 100;

/** `at` in the address, Unix milliseconds, as seconds; null when it is no moment. */
const momentOf = (text) => (/^[1-9]\d{0,14}$/.test(text ?? '') ? Math.floor(Number(text) / 1000) : null);

/** What the shell says for an address that no page answers, for a chart that is not one of the four. */
function noChart(view) {
  view.head({ title: 'No page here' });
  view.main.append(
    h('p', {}, 'The console has no page at ', h('span', { class: 'mono' }, location.pathname), '.'),
    h('div', { class: 'btns' }, h('a', { class: 'btn default', href: '/' }, 'Go to the Overview')));
}

/** What the chart says when the machine has kept no numbers: whether metricsd, which writes them, runs. `show` is the services' document. */
function noNumbersWords(show) {
  const metricsd = (Array.isArray(show?.services) ? show.services : []).find((service) => service.name === 'metricsd');
  if (metricsd && metricsd.state !== 'running') return `No numbers yet: metricsd is not running (${metricsd.state}).`;
  return 'No numbers yet. The first ones come within 10 minutes of metricsd starting.';
}

/** The range buttons of the head, which the arrow keys do not move between: each is a button of its own, pressed or not. */
function rangeControl(current, pick) {
  const buttons = RANGES.map((range) => h('button', {
    type: 'button', class: range.id === current ? 'on' : '', 'aria-pressed': String(range.id === current), 'data-range': range.id,
  }, range.label));
  for (const button of buttons) button.addEventListener('click', () => pick(button.dataset.range));
  return h('span', { class: 'seg', role: 'group', 'aria-label': 'Range' }, ...buttons);
}

/** A cell of a table: a number is right-aligned, words are not. */
const cell = (words, number = true) => h('td', { class: number ? 'num' : '' }, words);

/** A small table with a caption that only a screen reader reads; `numbers` is how many of its columns, the last ones, hold numbers. */
function table(caption, head, rows, numbers) {
  return h('div', { class: 'tbl-box' }, h('table', { class: 'tbl' },
    h('caption', { class: 'sr-only' }, caption),
    h('thead', {}, h('tr', {}, ...head.map((words, at) => h('th', { scope: 'col', class: at >= head.length - numbers ? 'num' : '' }, words)))),
    h('tbody', {}, ...rows)));
}

/** One item of the timeline: the board's, a time, a dot on the rail and the words. */
function timelineItem(item, time) {
  const said = { bad: 'Did not work', warn: 'Worth a look', info: '' }[item.dot];
  return h('div', { class: 'tl-item' },
    h('time', { datetime: new Date(item.at * 1000).toISOString(), title: time.full(item.at) }, time.when(item.at)),
    h('span', { class: 'tl-rail' }, h('i', { class: item.dot }, h('span', { class: 'sr-only' }, said))),
    h('div', { class: 'tl-body' }, item.kind === 'log' ? h('span', { class: 'tl-line' }, item.title) : h('b', {}, item.title), h('span', { class: 'muted small' }, item.sub)));
}

export async function show(view) {
  const [, id, ...extra] = view.rest.split('/');
  const chart = extra.length === 0 ? CHARTS.find((one) => one.id === id) : undefined;
  if (!chart) {
    noChart(view);
    return undefined;
  }

  let range = rangeOf(view.query.get('range')).id;
  // The moment chosen, in Unix seconds, and the step that holds it.
  let at = momentOf(view.query.get('at'));
  let step = null;
  let model = null;
  let journal = [];
  let facts = null;
  let records = [];
  let readGeneration = 0;
  let momentGeneration = 0;
  // Set when the server says the console is locked, though this page thought it was not: the person's own lines are read until the unlock changes.
  let lockedAnyway = false;
  let unlocked = isUnlocked();

  const mayReadAll = () => Boolean(view.session.admin) && unlocked && !lockedAnyway;
  const shownZone = () => view.time.zone;

  // ---- the head ----

  let control = null;
  function drawHead() {
    control = rangeControl(range, setRange);
    view.head({
      title: `${chart.short}, ${rangeOf(range).words.replace(/^the /, '')}`,
      crumbs: [{ title: 'Overview', path: `/?range=${range}` }],
      buttons: [control],
      commands: true,
    });
  }

  /** The address for the range and the moment now. */
  function remember() {
    const params = new URLSearchParams({ range });
    if (at !== null) params.set('at', String(at * 1000));
    history.replaceState(null, '', `${location.pathname}?${params}`);
  }

  // ---- the page ----

  const chartView = makeChart(chart.id, { pick: (seconds) => choose(seconds) });
  const chartStatus = h('div', { class: 'chartview-status' });
  const chosen = h('p', { class: 'small', role: 'status' });
  const chartCard = h('section', { class: 'card chartview-card', 'aria-label': `${chart.short} chart` }, chartStatus, chartView.element, chosen);
  const prompt = h('p', { class: 'muted', role: 'status' });

  const loading = () => h('p', { class: 'muted' }, 'Loading…');
  const happened = makeCard('happened', 'What happened around this moment');
  const busy = makeCard('busy', 'Busiest at this moment');
  const movedCard = makeCard('moved', 'What moved most');
  const logsCard = makeCard('moment-logs', 'Logs');
  const openInLogs = h('a', { class: 'btn link small', href: '/logs' }, 'Open in Logs with this time');
  logsCard.more.append(openInLogs);
  movedCard.more.textContent = 'compared with the hour before';
  const momentCards = h('div', { class: 'moment-cards' },
    h('div', { class: 'grid two' }, happened.element, busy.element),
    movedCard.element,
    logsCard.element);

  const who = h('div', { class: 'moment-who' });
  const commandSlot = h('div', { class: 'logs-command' });
  const logStatus = h('div', { class: 'logs-status' });
  const logNote = h('p', { class: 'small muted', role: 'status' });
  const list = makeLogList({
    label: 'Log lines of this moment, newest last',
    zones: () => [
      { label: 'Your zone', name: view.time.zone },
      ...(isZone(facts?.zone) ? [{ label: 'The server\'s zone', name: facts.zone }] : []),
      { label: 'UTC', name: 'UTC' },
    ],
    actions: (record) => {
      const text = lineOf(record, shownZone());
      const shownText = h('span', { class: 'sr-only', 'aria-hidden': 'true' }, text);
      const buttons = [h('a', { class: 'btn small', href: urlOf(aroundOf({ ...DEFAULT_QUERY, priority: 'warning' }, record)) }, 'Show 5 minutes around this')];
      if (record.from === 'service') buttons.push(h('a', { class: 'btn small', href: `/services/${encodeURIComponent(record.name)}` }, `Open ${record.name}`));
      buttons.push(h('button', {
        class: 'btn small', type: 'button', onclick: async () => view.notice((await copyText(text, shownText)) ? COPIED : PRESS_CTRL_C),
      }, 'Copy line'), shownText);
      return buttons;
    },
  });
  logsCard.body.replaceChildren(who, commandSlot, logStatus, list.element, logNote);

  view.main.append(h('div', { class: 'chartview' }, chartCard, prompt, momentCards));
  drawHead();

  // ---- words ----

  const stepWords = () => `${view.time.when(step.from)} to ${view.time.when(step.to)}`;

  function drawWho() {
    const says = whoWords(Boolean(view.session.admin), mayReadAll());
    if (!says) {
      who.replaceChildren();
      return;
    }
    const unlock = says.unlock ? h('button', { class: 'btn small', type: 'button' }, 'Unlock') : null;
    unlock?.addEventListener('click', () => askToUnlock(view.session));
    who.replaceChildren(h('p', { class: 'small muted' }, says.words, ' ', unlock));
  }

  function drawChosen() {
    chartView.setPicked(at === null || !model || model.empty ? null : step?.from ?? at);
    if (!model || model.empty || at === null || !step) {
      chosen.textContent = '';
      return;
    }
    const index = indexOfTime(model, step.from);
    chosen.replaceChildren('Chosen: ', h('b', {}, index === null ? view.time.when(step.from) : pickLabel(model, index, view.time)), '.');
  }

  // ---- the cards of the moment ----

  function hideMoment(words) {
    momentCards.hidden = true;
    prompt.textContent = words;
    prompt.hidden = words === '';
  }

  function fail(bodies, error, again) {
    for (const body of bodies) {
      body.replaceChildren(
        h('p', { class: 'muted', role: 'status' }, failureWords(error)),
        h('div', { class: 'btns' }, h('button', { class: 'btn small', type: 'button', onclick: again }, 'Try again')));
    }
  }

  function drawTimeline() {
    const every = rangeOf(range).every;
    const { items, more } = timelineOf({ changes: journal, records }, { from: step.from - every, to: step.to });
    happened.heading.firstChild.data = `What happened around ${view.time.when(step.from)}`;
    happened.more.textContent = `${view.time.when(step.from - every)} to ${view.time.when(step.to)}`;
    happened.body.replaceChildren(
      items.length === 0
        ? h('p', { class: 'muted' }, 'Nothing was written to the journal, and no warning or error to the log, in this time.')
        : h('div', { class: 'tl' }, ...items.map((item) => timelineItem(item, view.time))),
      ...(more > 0 ? [h('p', { class: 'small muted' }, `${more} earlier ${more === 1 ? 'item is' : 'items are'} in the journal and the log.`)] : []));
  }

  async function readAround(mine, now) {
    const bodies = [busy.body, movedCard.body];
    for (const body of bodies) body.replaceChildren(loading());
    let answer;
    try {
      answer = await view.api.get(`/api/1/metrics/around/${aroundAt(step, range, now)}?range=${range}`);
    } catch (error) {
      if (mine !== momentGeneration || error?.name === 'AbortError') return;
      // The command says it has no points when the machine has kept none: that is no failure.
      if (error?.id === 'ADM-123') answer = [];
      else {
        fail(bodies, error, () => readAround(mine, now));
        return;
      }
    }
    if (mine !== momentGeneration) return;
    const series = serviceSeries(answer);
    busy.heading.firstChild.data = `Busiest at ${view.time.when(step.from)}`;
    const rows = busiest(series, step);
    busy.body.replaceChildren(rows.length === 0
      ? h('p', { class: 'muted' }, 'No service has numbers for this moment.')
      : table('The services with the most CPU and the most memory', ['Service', 'CPU', 'Memory'], rows.map((row) => h('tr', {},
        h('th', { scope: 'row' }, row.service),
        cell(row.cpu === null ? 'no number' : formatPercent(row.cpu)),
        cell(row.memory === null ? 'no number' : formatBytes(row.memory)))), 2));
    movedCard.heading.firstChild.data = `What moved most from ${stepWords()}`;
    const found = moved(series, step);
    movedCard.body.replaceChildren(found.length === 0
      ? h('p', { class: 'muted' }, series.length === 0 ? 'No service has numbers for this moment.' : 'Nothing moved more than it does from one hour to the next.')
      : table('The services that moved most, against the hour before', ['Measure', 'Change'], found.map((row) => h('tr', {},
        h('th', { scope: 'row' }, row.name), cell(row.words, false))), 0));
  }

  async function readLogs(mine) {
    const every = rangeOf(range).every;
    const query = {
      ...DEFAULT_QUERY, when: 'custom', since: (step.from - every) * 1000, until: step.to * 1000, priority: 'warning',
    };
    const args = argsOf(query, { now: Date.now(), zone: shownZone(), lines: PAGE_LINES });
    const all = mayReadAll();
    logsCard.heading.firstChild.data = `Logs from ${stepWords()}, warnings and errors`;
    openInLogs.href = urlOf({
      ...DEFAULT_QUERY, when: 'custom', since: step.from * 1000, until: step.to * 1000, priority: 'warning',
    });
    drawWho();
    commandSlot.replaceChildren(view.api.inline(all ? 'logs.read-all' : 'logs.read', args));
    logStatus.replaceChildren();
    logNote.textContent = '';
    list.loading();
    let got;
    try {
      got = recordsOf(await view.api.get(readPath(args, { all })));
    } catch (error) {
      if (mine !== momentGeneration || error?.name === 'AbortError') return;
      if (all && (error.id === 'ADM-106' || error.id === 'ADM-122')) {
        lockedAnyway = true;
        readLogs(mine);
        return;
      }
      list.clear();
      list.element.hidden = true;
      records = [];
      drawTimeline();
      view.problem(error, { slot: logStatus, retry: () => readLogs(mine) });
      return;
    }
    if (mine !== momentGeneration) return;
    records = got?.records ?? [];
    drawTimeline();
    const inStep = records.filter((record) => {
      const seconds = timeOf(record.time)?.seconds;
      return seconds >= step.from && seconds < step.to;
    });
    list.element.hidden = inStep.length === 0;
    if (inStep.length === 0) {
      list.clear();
      logNote.textContent = 'No warning or error was logged in this time.';
      return;
    }
    list.setZone(shownZone());
    list.set(inStep);
    list.toBottom();
    // The read gives the newest lines: when its oldest is inside the step, older ones of the step are not here.
    if (records.length >= PAGE_LINES && records[0] === inStep[0]) logNote.textContent = `The newest ${PAGE_LINES} lines of this step. Open in Logs to see all.`;
  }

  async function loadMoment() {
    momentGeneration += 1;
    const mine = momentGeneration;
    if (!model || model.empty) {
      hideMoment('');
      return;
    }
    if (at === null) {
      step = null;
      hideMoment('Choose a moment on the chart, with the arrow keys and Enter or a click, to see what happened around it.');
      return;
    }
    const every = rangeOf(range).every;
    step = stepOf(at, every, timesOf(model));
    drawChosen();
    // A moment that the chart has no point for, a gap in the history or a time outside the range, has no
    // numbers; what the journal and the log hold of it still shows, as a change that stopped metricsd makes a gap.
    const numbered = timesOf(model).includes(step.from);
    hideMoment(numbered ? '' : `The history has no numbers for ${view.time.when(at)}.`);
    momentCards.hidden = false;
    happened.body.replaceChildren(loading());
    if (numbered) readAround(mine, Math.floor(Date.now() / 1000));
    else noNumbers();
    await readLogs(mine);
  }

  function noNumbers() {
    busy.heading.firstChild.data = `Busiest at ${view.time.when(step.from)}`;
    movedCard.heading.firstChild.data = `What moved most from ${stepWords()}`;
    for (const body of [busy.body, movedCard.body]) body.replaceChildren(h('p', { class: 'muted' }, 'The history has no numbers for this moment.'));
  }

  // ---- reading the chart ----

  async function readChart() {
    readGeneration += 1;
    const mine = readGeneration;
    const wanted = range;
    const now = Math.floor(Date.now() / 1000);
    chartStatus.replaceChildren();
    try {
      const [answer, about, rows] = await Promise.all([
        view.api.get(`/api/1/metrics/usage/${wanted}`).catch((error) => {
          // metrics says it has no points on a machine that has kept none, which is no failure.
          if (error?.id === 'ADM-123') return null;
          throw error;
        }),
        view.api.get('/api/1/console/about').catch(() => null),
        view.api.get(`/api/1/changes?since=${now - rangeOf(wanted).span}&limit=${JOURNAL_ROWS}`).catch(() => null),
      ]);
      if (mine !== readGeneration || view.signal.aborted) return;
      facts = about;
      journal = Array.isArray(rows?.changes) ? rows.changes : [];
      model = modelOf(chart, answer, { range: rangeOf(wanted), now, total: totalMemory(answer, about) });
      chartView.update(model, view.time);
      // A chart with no numbers has nothing to choose a moment from, so it marks nothing.
      chartView.setMarks(model.empty ? [] : marksOf(journal));
      if (model.empty) {
        const services = await view.api.get('/api/1/services').catch(() => null);
        if (mine !== readGeneration) return;
        chartStatus.replaceChildren(h('p', { class: 'muted', role: 'status' }, noNumbersWords(services)));
      }
    } catch (error) {
      if (mine !== readGeneration || error?.name === 'AbortError') return;
      view.problem(error, { slot: chartStatus, retry: readChart });
      return;
    }
    await loadMoment();
  }

  // ---- what a person does ----

  function choose(seconds) {
    at = seconds;
    remember();
    loadMoment();
  }

  function setRange(next) {
    if (next === range) return;
    range = next;
    const now = Math.floor(Date.now() / 1000);
    // A moment stays when the new range still holds it; its step is the new range's.
    if (at !== null && (at < now - rangeOf(range).span || at > now)) at = null;
    step = null;
    remember();
    drawHead();
    // The head is drawn again, and the person is where they were.
    control.querySelector(`[data-range="${range}"]`).focus();
    readChart();
  }

  const offUnlock = onUnlockChange(() => {
    const now = isUnlocked();
    if (now === unlocked) return;
    unlocked = now;
    lockedAnyway = false;
    if (step && !momentCards.hidden) readLogs(momentGeneration);
  });
  view.signal.addEventListener('abort', offUnlock, { once: true });

  await readChart();
  return undefined;
}
