// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The score and the checks (design 8.3): the ring, how many checks pass and
// when they ran, Check again, and each finding with what it protects against,
// what it takes off and its button. The kept result is for administrators
// (dukovo-check(8) keeps it so), so a reader is told that and nothing is read,
// as on the Overview; a server without dukovo-check says so. The page reads the kept result when
// it opens and when a change says that something moved, and runs the checks
// only when asked.

import { busy, fill, h } from '../../dom.js';
import { CHECK_NOTES } from '../../lib/checks.js';
import { copyText, COPIED, PRESS_CTRL_C } from '../../lib/copy.js';
import { assess } from '../../lib/health.js';
import { helpPath } from '../../lib/problem-model.js';
import { atMostEvery } from '../overview/refresh.js';
import { makeCard } from './card.js';
import {
  checkedWords, nothingRan, passedButton, passedRows, plusWords, pointsWords, ringWords, scoreWords,
} from './check-model.js';
import { firstSentence, fixesFor } from './model.js';

/** The least time between two reads that an event asks for. */
const EVENT_GAP_MS = 2000;

/** What the card says in place of a score. */
const NOTES = Object.freeze({
  missing: 'Checks are not on this server yet. This server shows the score with a later update of Dukovo.',
  none: 'The checks have not run on this server yet.',
  error: 'The checks could not be read.',
  reader: CHECK_NOTES.reader,
});

/** The ring, with its number and its band in words for a screen reader. */
function ring(assessed) {
  const element = h('div', { class: `score ${assessed.band.id}`, role: 'img', 'aria-label': ringWords(assessed.score, assessed.band) },
    h('span', {}, String(assessed.score), h('small', {}, 'of 100')));
  element.style.setProperty('--v', String(assessed.score));
  return element;
}

/** "Show how": the words of the advice and the command for a terminal, with Copy. */
function howOf(fix) {
  const status = h('span', { class: 'small', role: 'status' });
  const copy = fix.command && h('button', { class: 'btn small', type: 'button', onclick: async (event) => {
    status.textContent = (await copyText(fix.command, event.currentTarget)) ? COPIED : PRESS_CTRL_C;
  } }, 'Copy');
  const box = h('div', { class: 'sec-how', hidden: true }, h('p', { class: 'small' }, fix.words), fix.command && h('pre', { class: 'cmdtext', tabindex: '0' }, fix.command), h('div', { class: 'btns' }, copy, status));
  const open = h('button', { class: 'btn small', type: 'button', 'aria-expanded': 'false', onclick: () => {
    box.hidden = !box.hidden;
    open.setAttribute('aria-expanded', String(!box.hidden));
  } }, fix.label);
  return { open, box };
}

/**
 * The card. `hasPage(id)` says whether the console has the page `id`;
 * `openSsh(preset)` opens SSH's form for a finding about it. Gives
 * `{element, load}`.
 */
export function makeFindings(view, { hasPage, openSsh }) {
  const card = makeCard('checks', 'Health score');
  const again = h('button', { class: 'btn small', type: 'button', hidden: true, 'data-action': 'check-again' }, 'Check again');
  card.more.append(again);
  let kept = null;
  let state = 'loading';
  let showPassed = false;
  let latest = 0;

  /** One finding: its words, what it protects against, its points and its buttons. */
  function rowOf(finding) {
    const admin = view.session.admin === true;
    const fixes = admin ? fixesFor(finding, hasPage) : [];
    const boxes = [];
    const buttons = fixes.map((fix) => {
      if (fix.kind === 'link') return h('a', { class: 'btn small', href: fix.href }, fix.label);
      if (fix.kind === 'ssh') return h('button', { class: 'btn small', type: 'button', onclick: () => openSsh(fix.preset) }, fix.label);
      if (fix.kind === 'how') {
        const how = howOf(fix);
        boxes.push(how.box);
        return how.open;
      }
      return h('button', { class: 'btn small', type: 'button', onclick: async (event) => {
        await view.api.run(fix.action, fix.args, { button: event.currentTarget, words: fix.words });
      } }, fix.label);
    });
    const protects = firstSentence(finding.fix);
    return h('li', { class: 'sec-find' },
      h('div', { class: 'sec-find-words' },
        h('b', {}, h('span', { class: 'sr-only' }, finding.severity === 'bad' ? 'Failed: ' : 'Warning: '), finding.detail || finding.check),
        protects && h('span', { class: 'small muted' }, protects),
        finding.id && h('a', { class: 'small mono', href: helpPath(finding.id) }, finding.id)),
      h('div', { class: 'btns' },
        h('span', { class: `pts ${finding.severity}`, title: pointsWords(finding.points) }, plusWords(finding.points), h('span', { class: 'sr-only' }, ` ${pointsWords(finding.points)}`)),
        ...buttons),
      ...boxes);
  }

  function draw() {
    const admin = view.session.admin === true;
    again.hidden = !admin || state === 'loading' || state === 'missing';
    if (state === 'loading') return;
    card.ready();
    if (state !== 'ok') {
      const hint = state === 'none' ? ' Use Check again to run them.' : '';
      fill(card.body, h('p', { class: 'muted' }, `${NOTES[state]}${hint}`));
      return;
    }
    const assessed = assess(kept);
    const passed = passedRows(kept);
    const list = h('ul', { class: 'sec-passed', hidden: !showPassed }, ...passed.map((row) => h('li', {}, h('span', { class: `st ${row.skipped ? 'off' : 'ok'}` }, row.skipped ? 'Skipped' : 'Passed'), ` ${row.check}`, row.detail && h('span', { class: 'small muted' }, ` ${row.detail}`))));
    const toggle = passed.length > 0 && h('button', { class: 'btn link small', type: 'button', 'aria-expanded': String(showPassed), onclick: (event) => {
      showPassed = !showPassed;
      list.hidden = !showPassed;
      event.currentTarget.setAttribute('aria-expanded', String(showPassed));
    } }, passedButton(passed));
    fill(card.body,
      h('div', { class: 'sec-score' },
        ring(assessed),
        h('div', { class: 'sec-score-words' },
          h('span', { class: `st ${assessed.band.id}` }, assessed.band.words),
          h('span', {}, scoreWords(kept)),
          assessed.checked !== null && h('span', { class: 'small muted' }, `Checked ${checkedWords(assessed.checked, view.time)}`))),
      assessed.findings.length > 0
        ? h('ul', { class: 'sec-finds' }, ...assessed.findings.map(rowOf))
        : h('p', { class: 'muted' }, 'Nothing needs you.'),
      toggle, list);
  }

  /** Reads the kept result. Only the last read counts. */
  async function read() {
    latest += 1;
    if (view.session.admin !== true) {
      state = 'reader';
      draw();
      return;
    }
    const mine = latest;
    try {
      const result = await view.api.get('/api/1/checks/last');
      if (mine !== latest) return;
      kept = result;
      state = nothingRan(result) ? 'none' : 'ok';
      fill(card.errors);
    } catch (error) {
      if (mine !== latest || error?.name === 'AbortError') return;
      if (error?.id === 'ADM-129') state = 'missing';
      else if (error?.id === 'ADM-123') state = 'none';
      else {
        state = 'error';
        view.problem(error, { slot: card.errors });
      }
    }
    draw();
  }

  again.addEventListener('click', async () => {
    const result = await view.api.run('checks.run', {}, {
      button: again, where: { slot: card.errors }, words: { unlock: 'run the checks', running: 'Running the checks', done: 'The checks ran.' },
    });
    if (!result.ok) return;
    // The answer is the result that was kept; one that is not is read again.
    if (!nothingRan(result.result)) {
      kept = result.result;
      state = 'ok';
      draw();
    } else {
      await busy([again], read);
    }
  });

  const soon = atMostEvery(read, EVENT_GAP_MS);
  view.signal.addEventListener('abort', soon.stop, { once: true });
  view.on('change', soon);
  return { element: card.element, load: read };
}
