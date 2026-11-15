// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Logs page (design 5): the lines of the machine's log, at `/logs` with
// the query in the address. An unlocked administrator reads the whole log
// (`logs.read-all`, each read one audit line); anyone else reads their own
// programs' lines (`logs.read`). The page reads when it opens, when a filter
// changes, on Read again and when the unlock changes, never on a timer; to
// see new lines it follows the log, which is one stream (follow.js).
// `/logs/chart/{chart}` is the view from a chart (chart-view.js).

import { h, icon } from '../../dom.js';
import { copyText, COPIED, PRESS_CTRL_C } from '../../lib/copy.js';
import { makeLogList } from '../../lib/log-list.js';
import {
  aroundOf, argsOf, followPath, lineOf, MOST_LINES, needsRead, olderArgs, onlyFrom, PRIORITIES, queryOf, readPath, recordsOf, saveName, secondOf, sourceOf, urlOf, whoWords, zoneId,
} from '../../lib/log-model.js';
import { saveText } from '../../lib/save-file.js';
import { isZone } from '../../lib/time.js';
import { RANGES, timeOf, wallOf } from '../../lib/when-model.js';
import { askToUnlock, isUnlocked, onUnlockChange } from '../../unlock.js';
import { makeFilters } from './filters.js';
import { makeFollower, openFollow } from './follow.js';

/** How long lines that come while following wait before they are drawn, so that a busy log is drawn a few times a second and not for each line. */
const BATCH_MS = 250;

/** A banner of `kind` with an icon, `words` and buttons. */
function banner(kind, name, words, buttons = []) {
  return h('div', { class: `banner ${kind}`, role: 'status' },
    icon(name),
    h('div', {}, h('p', {}, words), buttons.length > 0 && h('div', { class: 'btns' }, ...buttons)));
}

/** The range after `when`, as a longer time: null when there is none. */
function longer(when) {
  const order = ['15m', '1h', '24h', '7d'];
  const at = order.indexOf(when);
  if (at >= 0) return order[at + 1] ?? null;
  return '7d';
}

/** The importance after `priority`, as a less strict one: null when everything is shown already. */
function lessStrict(priority) {
  const order = PRIORITIES.map((one) => one.id);
  return order[order.indexOf(priority) + 1] ?? null;
}

export async function show(view) {
  if (view.rest.startsWith('chart/')) return (await import('./chart-view.js')).show(view);
  if (view.rest !== '') {
    view.go('/logs');
    return undefined;
  }

  const { query: given, refused } = queryOf(view.query);
  let query = given;
  let readAt = Date.now();
  let generation = 0;
  let serverZone = null;
  let unlocked = isUnlocked();
  // Set when the server says the console is locked, though this page thought it was not: the person's own lines are read until the unlock changes.
  let lockedAnyway = false;
  let waiting = [];
  let batch = null;
  let leftOut = 0;

  // ---- who reads what (5.3) ----

  const mayReadAll = () => Boolean(view.session.admin) && unlocked && !lockedAnyway;
  const names = () => ({ you: view.time.zone, server: serverZone });
  const shownZone = () => zoneId(query.zone, names()) ?? view.time.zone;

  // ---- the page ----

  const followButton = h('button', { class: 'btn', type: 'button' }, 'Follow new lines');
  const saveButton = h('button', { class: 'btn', type: 'button', disabled: true }, 'Save as file…');
  const live = h('span', { class: 'pill ok', hidden: true }, 'Live');
  view.head({ title: 'Logs', commands: true, buttons: [live, followButton, saveButton] });

  const who = h('div', { class: 'logs-who' });
  const filters = makeFilters({
    query, zoneNames: names, signal: view.signal, onChange: (change) => setQuery({ ...query, ...change }),
  });
  const commandSlot = h('div', { class: 'logs-command' });
  const status = h('div', { class: 'logs-status' });
  // What a follow that ended says stays while the list is read again, until a follow begins.
  const stopped = h('div', { class: 'logs-status' });
  const olderButton = h('button', { class: 'btn small', type: 'button', hidden: true }, 'Show older lines');
  const olderNote = h('span', { class: 'small muted', role: 'status' });
  const readAgain = h('button', { class: 'btn small', type: 'button' }, 'Read again');
  const addressNote = h('p', { class: 'small muted', role: 'status' });
  const leftNote = h('p', { class: 'small muted', role: 'status' });
  const footNote = h('p', { class: 'small muted' });

  const list = makeLogList({
    zones: () => [
      { label: 'Your zone', name: view.time.zone },
      ...(serverZone ? [{ label: 'The server\'s zone', name: serverZone }] : []),
      { label: 'UTC', name: 'UTC' },
    ],
    actions: (record, { close }) => rowButtons(record, close),
  });

  view.main.append(h('div', { class: 'logs' },
    who,
    addressNote,
    filters.element,
    commandSlot,
    stopped,
    status,
    h('div', { class: 'logs-bar' }, olderButton, olderNote, h('span', { class: 'spacer' }), readAgain),
    list.element,
    leftNote,
    footNote));

  // ---- the banner of 5.3 ----

  function drawWho() {
    const says = whoWords(Boolean(view.session.admin), mayReadAll());
    if (!says) {
      who.replaceChildren();
    } else if (says.unlock) {
      const unlock = h('button', { class: 'btn small', type: 'button' }, 'Unlock');
      unlock.addEventListener('click', () => askToUnlock(view.session));
      who.replaceChildren(banner('info', 'lock', says.words, [unlock]));
    } else {
      who.replaceChildren(banner('info', 'info', says.words));
    }
  }

  function drawFoot() {
    const zone = shownZone();
    footNote.textContent = `Times are in ${query.zone === 'utc' ? 'UTC' : zone}. Folded lines show how often they repeated. New lines scroll into view while you are at the bottom of the list; scroll up to stop.`;
    saveButton.disabled = list.records().length === 0;
  }

  // ---- the command ----

  function drawCommand(args, all) {
    followLine = null;
    commandSlot.replaceChildren(view.api.inline(all ? 'logs.read-all' : 'logs.read', args));
  }

  // The command a follow runs is shown as the Show commands switch says, as the read's is.
  let followLine = null;
  view.onPrefs((prefs) => { if (followLine) followLine.hidden = prefs.commands !== 'shown'; });

  function drawFollowCommand(shown) {
    followLine = h('p', { class: 'inline-cmd', hidden: view.prefs?.commands !== 'shown' }, shown);
    commandSlot.replaceChildren(followLine);
  }

  // ---- reading ----

  function empty() {
    const choices = [];
    const wider = longer(query.when);
    const less = lessStrict(query.priority);
    if (wider) {
      const words = `Try the ${RANGES.find((range) => range.id === wider).label.toLowerCase()}`;
      choices.push(h('button', { class: 'btn small', type: 'button', onclick: () => setQuery({ ...query, when: wider, since: null, until: null }) }, words));
    }
    if (less) {
      const words = `Show ${PRIORITIES.find((one) => one.id === less).label.toLowerCase()}`;
      choices.push(h('button', { class: 'btn small', type: 'button', onclick: () => setQuery({ ...query, priority: less }) }, words));
    }
    status.replaceChildren(h('div', { class: 'logs-empty', role: 'status' },
      h('p', {}, 'No lines match. Try a longer time or less importance.'),
      choices.length > 0 && h('div', { class: 'btns' }, ...choices)));
  }

  function failed(error, all) {
    if (error?.name === 'AbortError') return;
    // The console is locked after all, or this account is no administrator: the person's own lines.
    if (all && (error.id === 'ADM-106' || error.id === 'ADM-122')) {
      lockedAnyway = true;
      drawWho();
      read();
      return;
    }
    list.clear();
    list.element.hidden = true;
    view.problem(error, { slot: status, fields: filters.fields, retry: read });
    drawFoot();
  }

  async function read(reveal = null) {
    const mine = generation + 1;
    generation = mine;
    readAt = Date.now();
    const all = mayReadAll();
    const args = argsOf(query, { now: readAt, zone: shownZone() });
    status.replaceChildren();
    olderNote.textContent = '';
    olderButton.hidden = true;
    leftNote.textContent = '';
    list.element.hidden = false;
    list.loading();
    if (!following()) drawCommand(args, all);
    let answer;
    try {
      answer = await view.api.get(readPath(args, { all }));
    } catch (error) {
      if (mine === generation) failed(error, all);
      return;
    }
    if (mine !== generation) return;
    const got = recordsOf(answer);
    if (!got) {
      failed(new Error('The server did not answer with a list of lines.'), false);
      return;
    }
    list.set(got.records);
    if (got.records.length === 0) {
      list.element.hidden = true;
      empty();
    } else if (reveal) {
      list.reveal(reveal);
    }
    olderButton.hidden = got.records.length < args.lines;
    drawFoot();
  }

  async function older() {
    const oldest = list.records()[0];
    const args = oldest && olderArgs(query, oldest, { now: readAt, zone: shownZone() });
    if (!args) return;
    olderButton.disabled = true;
    olderNote.textContent = '';
    let answer;
    try {
      answer = await view.api.get(readPath(args, { all: mayReadAll() }));
    } catch (error) {
      olderButton.disabled = false;
      view.problem(error, { slot: status, button: olderButton, retry: older });
      return;
    }
    olderButton.disabled = false;
    const got = recordsOf(answer);
    if (!got) return;
    const added = list.merge(got.records);
    if (got.records.length === 0) {
      olderButton.hidden = true;
      olderNote.textContent = 'These are the oldest lines in this time.';
    } else if (added === 0) {
      // More lines share the second of the oldest one than a page holds.
      const second = wallOf(timeOf(oldest.time).ms, shownZone()).time;
      olderNote.replaceChildren(`More lines at ${second} than one page holds. `, h('a', { href: urlOf(secondOf(query, oldest)) }, 'Open that second alone'));
      olderButton.hidden = true;
    } else if (got.records.length < args.lines) {
      olderButton.hidden = true;
      olderNote.textContent = 'These are the oldest lines in this time.';
    }
    drawFoot();
  }

  // ---- the query ----

  const followChanged = (before, after) => ['service', 'kernel', 'priority', 'q'].some((name) => before[name] !== after[name]);

  function setQuery(next, reveal = null) {
    const before = query;
    query = next;
    history.replaceState(null, '', urlOf(query));
    filters.set(query);
    list.setZone(shownZone());
    list.setFolded(query.fold);
    if (needsRead(before, query)) {
      read(reveal);
      if (following() && followChanged(before, query)) follower.restart();
    }
    drawFoot();
  }

  // ---- the buttons under an opened row (5.4) ----

  function rowButtons(record, close) {
    const text = lineOf(record, shownZone());
    const buttons = [
      h('button', { class: 'btn small', type: 'button', onclick: () => { close(); setQuery(aroundOf(query, record), record); } }, 'Show 5 minutes around this'),
    ];
    if (record.from === 'service') buttons.push(h('a', { class: 'btn small', href: `/services/${encodeURIComponent(record.name)}` }, `Open ${record.name}`));
    if (sourceOf(record)) {
      buttons.push(h('button', { class: 'btn small', type: 'button', onclick: () => { close(); setQuery(onlyFrom(query, record)); } }, record.from === 'kernel' ? 'Only the machine itself' : 'Only this service'));
    }
    const shown = h('span', { class: 'sr-only', 'aria-hidden': 'true' }, text);
    buttons.push(h('button', {
      class: 'btn small',
      type: 'button',
      onclick: async () => view.notice((await copyText(text, shown)) ? COPIED : PRESS_CTRL_C),
    }, icon('copy'), 'Copy line'), shown);
    return buttons;
  }

  // ---- saving ----

  saveButton.addEventListener('click', () => {
    const zone = shownZone();
    const lines = list.records().map((record) => lineOf(record, zone));
    if (lines.length === 0) return;
    const name = saveName(view.host, Date.now(), zone);
    saveText(name, `${lines.join('\n')}\n`);
    view.notice(`Saved ${lines.length} ${lines.length === 1 ? 'line' : 'lines'} as ${name}.`);
  });

  // ---- following (5.5) ----

  function flush() {
    batch = null;
    const records = waiting;
    waiting = [];
    if (records.length > 0) list.merge(records, { live: true });
    saveButton.disabled = list.records().length === 0;
  }

  function onEvent(event) {
    if (event.kind === 'command') {
      drawFollowCommand(event.shown);
    } else if (event.kind === 'record') {
      waiting.push(event.record);
      batch ??= setTimeout(flush, BATCH_MS);
    } else if (event.kind === 'skipped') {
      leftOut += event.count;
      leftNote.textContent = `${leftOut} lines were left out because more than 100 came in a second.`;
    }
  }

  /** Reads the lines that came while the tab was hidden, from the last line the list has. */
  async function readGap() {
    const last = list.records().at(-1);
    const since = last ? timeOf(last.time).ms : readAt;
    const args = argsOf({
      ...query, when: 'custom', since, until: null, lines: MOST_LINES,
    }, { now: Date.now(), zone: shownZone() });
    const got = recordsOf(await view.api.get(readPath(args, { all: mayReadAll() })));
    if (!got) return;
    list.merge(got.records, { live: true });
    if (got.records.length >= MOST_LINES) leftNote.textContent = 'More than 1,000 lines came while this tab was hidden, and the oldest of them are not shown. Read again to see them all.';
  }

  const follower = makeFollower({
    open: (signal, on) => openFollow({ path: followPath(query, { all: mayReadAll() }), signal, on }),
    readGap,
    onEvent,
    onState(state, detail) {
      live.hidden = state !== 'on';
      followButton.textContent = state === 'on' || state === 'away' ? 'Pause' : 'Follow new lines';
      if (state === 'on') {
        stopped.replaceChildren();
        status.replaceChildren();
        leftOut = 0;
        leftNote.textContent = '';
      } else {
        clearTimeout(batch);
        flush();
        list.stopCounting();
        drawCommand(argsOf(query, { now: readAt, zone: shownZone() }), mayReadAll());
      }
      if (state !== 'stopped') return;
      const followAgain = h('button', { class: 'btn small', type: 'button' }, 'Follow again');
      followAgain.addEventListener('click', () => follower.start());
      if (detail.error) {
        view.problem(detail.error, { slot: stopped, button: followButton, retry: () => follower.start() });
      } else {
        stopped.replaceChildren(banner('warn', 'warn', detail.words, [followAgain]));
      }
    },
    visible: () => document.visibilityState === 'visible',
  });
  const following = () => follower.state() === 'on' || follower.state() === 'away';

  followButton.addEventListener('click', () => (following() ? follower.stop() : follower.start()));
  const onVisibility = () => (document.visibilityState === 'visible' ? follower.shown() : follower.hidden());
  document.addEventListener('visibilitychange', onVisibility);

  // ---- the rest ----

  olderButton.addEventListener('click', older);
  readAgain.addEventListener('click', () => read());

  const offUnlock = onUnlockChange(() => {
    const now = isUnlocked();
    if (now === unlocked) return;
    unlocked = now;
    lockedAnyway = false;
    drawWho();
    read();
    // Locking ends a follow of the whole log by itself, with its sentence; unlocking widens one that goes on.
    if (now && following()) follower.restart();
  });
  view.signal.addEventListener('abort', () => {
    offUnlock();
    document.removeEventListener('visibilitychange', onVisibility);
    clearTimeout(batch);
    follower.dispose();
    list.stopCounting();
  }, { once: true });

  drawWho();
  list.setZone(shownZone());
  list.setFolded(query.fold);
  if (refused.length > 0) {
    addressNote.textContent = `This link had filters that are not used: ${refused.join(', ')}.`;
  }

  // The server's zone, for Times in; and the names of the services, for From. Neither holds up the lines.
  const about = view.api.get('/api/1/console/about').then((answer) => {
    serverZone = isZone(answer?.zone) ? answer.zone : null;
  }, () => {});
  view.api.get('/api/1/services').then((answer) => {
    filters.setServices((Array.isArray(answer?.services) ? answer.services : []).map((one) => one.name).filter((name) => typeof name === 'string').sort());
  }, () => {});
  if (query.zone === 'server') await about;
  about.then(() => {
    if (query.zone === 'server' && !serverZone) query = { ...query, zone: 'you' };
    filters.set(query);
    list.setZone(shownZone());
    drawFoot();
  });
  await read();
  return undefined;
}
