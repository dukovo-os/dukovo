// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The banner of a tried change (design 8.4 to 8.9), which Networking and
// Firewall draw on every route of the page: the countdown on the server's
// clock, who began the trial, whether this browser reaches the console over
// the new settings, Keep this change and Undo now. It shows the trial of
// Spark's document, so another session's try and one begun at a terminal are
// seen as well. The model is lib/trial-model.js; the sheet is
// lib/trial-banner.css, which pages.js loads for each of the two pages.

import { h, icon } from '../dom.js';
import {
  announceAt, begunWords, clockWords, endedHow, leftWords, makeReach, momentWords, percentLeft, reachWords, trialOf, triedBy,
  undoneAt,
} from './trial-model.js';

/** How often the countdown is drawn, and how often the document is read again to correct it (design 8.9). */
const TICK_MS = 1000;
const READ_MS = 5000;

/** How long after the time is out the document is read once more, since Spark's undo takes a moment. */
const SETTLE_MS = 1500;

/** How far the end of a trial moves between two reads before it is a new end, in milliseconds. */
const ENDS_MOVED_MS = 2000;

/** The journal's newest rows that are read to find who began a trial. */
const JOURNAL_ROWS = 100;

/** A timer that gives what cancels it, as lib/trial-model.js's machine takes it. */
const later = (fn, ms) => {
  const timer = setTimeout(fn, ms);
  return () => clearTimeout(timer);
};

/** Whether the person asked for less motion, in the console's choice or the device's. */
const calm = (view) => view.prefs?.motion === 'less' || matchMedia('(prefers-reduced-motion: reduce)').matches;

/**
 * The banner of the trial `id` (`network` or `firewall`). `what(card)` is the
 * sentence that says what is being tried, `card` being the network card the
 * journal names or null. `keep` and `undo` are the actions' ids.
 * `onState({running, ended})` is called when a trial is first seen, and when it
 * has ended, with `ended` as `{how: 'kept' | 'undone' | 'unknown', why}`, so that the page
 * reads its own data again and turns its form on or off.
 *
 * Gives `{element, begin, lost, read, running}`: `begin({seconds, card,
 * url})` is called when this page's try was answered, `lost` with the same
 * and `sentAt` when the try took away the address this browser comes by
 * and no answer came, `read()` reads the document, and `running()` says
 * whether a trial runs.
 */
export function makeTrialBanner(view, {
  id, what, keep, undo, onState = () => {},
}) {
  const clock = h('span', { class: 'count-down', 'aria-hidden': 'true' });
  const meterFill = h('i');
  const heading = h('h2', { class: 'try-h', tabindex: '-1' });
  const words = h('p', { class: 'try-words' });
  const link = h('p', { class: 'try-link', hidden: true });
  const applied = h('p', { class: 'try-step', hidden: true }, h('span', { class: 'st ok' }, 'Applied'));
  const reachLine = h('p', { class: 'try-step', hidden: true });
  const reason = h('span', { id: `try-reason-${id}`, class: 'small muted try-reason' });
  const keepButton = h('button', { class: 'btn default', type: 'button', 'aria-describedby': reason.id }, 'Keep this change');
  const undoButton = h('button', { class: 'btn', type: 'button' }, 'Undo now');
  // Only an administrator keeps or undoes a change: anyone else is told who does.
  const readerNote = h('p', { class: 'small muted try-reader', hidden: true }, 'Administrators keep or undo the change.');
  const banner = h('div', { class: 'banner warn try', role: 'status' },
    icon('warn'),
    h('div', {},
      h('div', { class: 'try-head' }, clock, h('div', { class: 'try-text' }, heading, words)),
      link, applied, reachLine,
      h('div', { class: 'meter warn', 'aria-hidden': 'true' }, meterFill),
      h('div', { class: 'btns' }, keepButton, undoButton, reason),
      readerNote));
  const over = h('div', { class: 'banner try-over', role: 'status', hidden: true }, icon('info'), h('div', {}, h('p', { class: 'try-over-words' })));
  const polite = h('p', { class: 'sr-only', role: 'status', 'aria-live': 'polite' });
  const box = h('div', { class: 'try-box', hidden: true }, banner, over);
  // The polite region is always there, so that what it says when the box appears is read.
  const element = h('div', { class: 'try-wrap' }, box, polite);

  // What is known of the trial that runs.
  let anchor = null; // {leftMs, at, endsMs}: the time left when the document was read, when, and the end by this browser's clock
  let windowMs = 0;
  let began; // undefined: not read; null: at a terminal; else the journal's row's `{who, how, seconds, card}`
  let mine = false;
  // A trial this page began that no read has shown yet: the reach starts once one does.
  let awaited = false;
  // A trial this page began whose answer did not come, shown by this browser's clock until a read shows it.
  let unseen = false;
  let begunCard = null;
  let undoneBefore = [];
  let lastShow = null;
  let reachState = { kind: 'idle' };
  let spoken = null;
  let settled = false;
  let latest = 0;

  const ends = () => view.time.clock(Math.floor(anchor.endsMs / 1000));
  const leftNow = () => (anchor === null ? 0 : anchor.leftMs - (performance.now() - anchor.at));

  /** Sets `node`'s text only when it differs, so that a region that is read aloud is not read again for nothing. */
  const say = (node, text) => {
    if (node.textContent !== text) node.textContent = text;
  };

  const reach = makeReach({
    ask: () => view.api.get(`/api/1/console/fresh/${id}`),
    schedule: later,
    over: () => anchor === null || leftNow() <= 0,
    onChange: (state) => {
      reachState = state;
      draw();
    },
  });

  /** Makes the keep button on or off; it stays in the order of keys either way, and the reason says why. */
  function setKeep(wanted) {
    keepButton.setAttribute('aria-disabled', String(wanted !== null));
    say(reason, wanted ?? '');
  }

  /** Draws what changes with the reach: its line, and whether Keep is on. */
  function draw() {
    if (anchor === null) return;
    const admin = view.session.admin === true;
    readerNote.hidden = admin;
    keepButton.hidden = !admin;
    undoButton.hidden = !admin;
    reason.hidden = !admin;
    const said = reachWords(reachState, ends());
    // A page that did not begin the trial checks only when Keep is pressed.
    const asked = mine || reachState.kind !== 'idle';
    applied.hidden = !mine || unseen;
    reachLine.hidden = !asked;
    if (asked) {
      reachLine.className = `try-step${reachState.kind === 'reached' ? ' ok' : ''}`;
      say(reachLine, said.words);
    }
    setKeep(mine ? said.keep : (asked && said.keep !== null && reachState.kind !== 'reached' ? said.keep : null));
  }

  /** Draws the countdown, the words that carry the end time and the meter, and says the moments aloud. */
  function drawTime() {
    if (anchor === null) return;
    const left = Math.max(0, leftNow());
    say(clock, clockWords(left));
    meterFill.style.setProperty('--v', String(percentLeft(left, windowMs, calm(view))));
    const sentence = began === undefined ? '' : ` ${began === null ? 'Begun at a terminal.' : `Begun ${begunWords(began)}.`}`;
    say(words, `The change is undone at ${ends()} unless it is kept.${sentence}`);
    const moment = announceAt(left);
    if (moment !== null && moment !== spoken && left > 0) {
      spoken = moment;
      say(polite, momentWords(moment));
    }
    if (left <= 0 && !settled) {
      settled = true;
      setTimeout(read, SETTLE_MS);
    }
  }

  /** The words of the heading, from what the journal says of the card. */
  const drawWhat = () => say(heading, what(mine ? begunCard : began?.card ?? null));

  /** The link to the new address, when the page's change takes this one away. */
  function drawLink(url) {
    link.hidden = url === null || url === undefined;
    if (!link.hidden) link.replaceChildren('Open the console at its new address: ', h('a', { href: url, target: '_blank', rel: 'noopener' }, url));
  }

  /** Shows `outcome` where the trial was and tells the page. */
  function finish(outcome) {
    reach.stop();
    anchor = null;
    mine = false;
    awaited = false;
    unseen = false;
    windowMs = 0;
    banner.hidden = true;
    box.hidden = false;
    const text = {
      undone: `The change was undone: ${outcome.why}.`,
      kept: 'The change was kept.',
      unknown: 'This browser cannot tell whether the change was made: the page shows the settings the machine has now.',
    }[outcome.how];
    over.className = `banner try-over ${outcome.how === 'kept' ? 'ok' : 'warn'}`;
    over.hidden = false;
    over.querySelector('.try-over-words').textContent = text;
    say(polite, text);
    onState({ running: false, ended: outcome });
  }

  /** Who began a trial that this page did not: from the journal. A journal that cannot be read says nothing. */
  async function findWho() {
    try {
      const rows = await view.api.get(`/api/1/changes?limit=${JOURNAL_ROWS}`);
      began = triedBy(rows?.changes, id);
    } catch {
      began = undefined;
    }
    if (began?.seconds) windowMs = Math.max(windowMs, began.seconds * 1000);
    drawWhat();
    drawTime();
  }

  /** A trial of the document: the first time it is seen starts the banner. */
  function seen(trial, show) {
    const first = anchor === null;
    unseen = false;
    // Spark's clock counts from the machine's start, so the end is this
    // browser's now and the time left. It moves only when the trial's end
    // does (by more than a read's delay), so that the words of the end time
    // do not change back and forth.
    const endsMs = Date.now() + trial.left_ms;
    const moved = first || Math.abs(endsMs - anchor.endsMs) > ENDS_MOVED_MS;
    anchor = { leftMs: trial.left_ms, at: performance.now(), endsMs: moved ? endsMs : anchor.endsMs };
    settled = false;
    if (first) {
      over.hidden = true;
      banner.hidden = false;
      box.hidden = false;
      undoneBefore = undoneAt(show, id);
      windowMs = Math.max(windowMs, trial.left_ms);
      spoken = null;
      say(polite, `${leftWords(trial.left_ms)} left to keep the change.`);
      drawWhat();
      drawTime();
      draw();
      onState({ running: true, ended: null });
      if (!mine) findWho();
      startReach();
      return;
    }
    drawTime();
    draw();
    startReach();
  }

  /**
   * Once a read shows the trial this page began: asks whether this browser
   * reaches the console, and takes focus. The read `begin` makes may lose to
   * another one that the page's own reads start, so any read that sees the
   * trial first does this.
   */
  function startReach() {
    if (!awaited || anchor === null) return;
    awaited = false;
    drawWhat();
    drawTime();
    reach.start();
    heading.focus();
  }

  /** Reads Spark's document; what it says is the truth of the trial, whoever began it. */
  async function read() {
    latest += 1;
    const mineRead = latest;
    let show;
    try {
      show = await view.api.get('/api/1/services');
    } catch {
      // Nothing is drawn again on a read that failed: the countdown goes on by the clock, which is what the undo does too.
      return;
    }
    if (mineRead !== latest) return;
    lastShow = show;
    const trial = trialOf(show, id);
    if (trial !== null) seen(trial, show);
    else if (anchor !== null) finish(ended(show));
  }

  /**
   * How the trial ended, by the document that no longer has it. A trial
   * that no read ever showed was undone if the document says so, and
   * otherwise may have been kept or never made.
   */
  function ended(show) {
    const how = endedHow(show, id, undoneBefore);
    return unseen && how.how === 'kept' ? { how: 'unknown' } : how;
  }

  /** The page's try was answered: this banner watches the trial, asks whether this browser reaches the console, and takes focus. */
  async function begin({ seconds, card = null, url = null }) {
    mine = true;
    begunCard = card;
    began = { who: view.session.name, how: 'console', seconds, card };
    windowMs = seconds * 1000;
    if (lastShow !== null) undoneBefore = undoneAt(lastShow, id);
    reachState = { kind: 'idle' };
    drawLink(url);
    awaited = true;
    // The document may show the trial already, which is as it should be.
    await read();
    startReach();
  }

  /**
   * The page's try took away the address this browser comes by, and its
   * answer did not come back by it: the change most likely runs. The banner
   * shows it as this page asked for it, by this browser's clock from
   * `sentAt` (a `performance.now()`), the time the try was sent, and asks
   * whether this browser reaches the console, until a read can say more.
   */
  function lost({ seconds, sentAt, card = null, url = null }) {
    mine = true;
    unseen = true;
    begunCard = card;
    began = { who: view.session.name, how: 'console', seconds, card };
    windowMs = seconds * 1000;
    if (lastShow !== null) undoneBefore = undoneAt(lastShow, id);
    anchor = { leftMs: windowMs, at: sentAt, endsMs: Date.now() - (performance.now() - sentAt) + windowMs };
    settled = false;
    spoken = null;
    reachState = { kind: 'idle' };
    awaited = true;
    over.hidden = true;
    banner.hidden = false;
    box.hidden = false;
    drawLink(url);
    say(polite, `${leftWords(windowMs)} left to keep the change.`);
    draw();
    onState({ running: true, ended: null });
    startReach();
  }

  /** Keep: for a page that did not begin the trial, after one round that asks whether this browser reaches the console. */
  async function pressKeep() {
    // Keep that is off stays where the keys reach, and its reason is read with it.
    if (keepButton.getAttribute('aria-disabled') === 'true') return;
    if (!mine) {
      reachState = { kind: 'asking' };
      draw();
      const found = await reach.check();
      if (anchor === null) return;
      reachState = found;
      draw();
      if (found.kind !== 'reached') {
        if (found.kind === 'gone') read();
        return;
      }
    }
    const result = await view.api.run(keep, {}, {
      button: keepButton, words: { unlock: 'keep the change', running: 'Keeping the change', done: 'The change is kept.' },
    });
    if (result.ok) finish({ how: 'kept' });
    else read();
  }

  async function pressUndo() {
    const result = await view.api.run(undo, {}, {
      button: undoButton, words: { unlock: 'undo the change', running: 'Undoing the change', done: 'The change is undone.' },
    });
    if (result.ok) finish({ how: 'undone', why: 'you undid it' });
    else read();
  }

  keepButton.addEventListener('click', pressKeep);
  undoButton.addEventListener('click', pressUndo);

  const ticker = setInterval(drawTime, TICK_MS);
  view.signal.addEventListener('abort', () => {
    clearInterval(ticker);
    reach.stop();
  }, { once: true });
  view.every(READ_MS, () => {
    if (anchor !== null) read();
  });
  view.on('change', read);

  return {
    element,
    begin,
    lost,
    read,
    running: () => anchor !== null,
  };
}
