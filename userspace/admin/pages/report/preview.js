// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// "Exactly what is sent": the text as reportd keeps it, a row for each line,
// with what redaction took out and what the person cut marked. A line the
// system gathered shows Cut line when it is pointed at or reached with the
// arrow keys, and words or lines selected in the text show a button beside
// the selection. The page keeps the state; this draws the text and says what
// was asked of it.

import {
  fill, h, uniqueId,
} from '../../dom.js';
import {
  drafted, gatheredOf, linesPick, piecesOf, rowsOf, withLines, withWords, wordsPick,
} from './cuts-model.js';
import {
  holdsWords, marksWords, textLabel, youCutWords,
} from './model.js';
import { selectedIn } from './selection.js';

const WHEN_NONE = { choose: 'Choose what went wrong to see the report.', drafting: 'Drafting…' };

/**
 * Makes the card. `onCut(cuts)` asks for the text with `cuts`, the whole list;
 * `onAnswers(answers)` for the answers written again, true when it was made;
 * `onUndo()` and `onAgain()` are the buttons of those names. `readSelection`
 * is selection.js's, which a test replaces.
 */
export function makePreview(view, {
  onCut, onAnswers, onUndo, onAgain, readSelection = selectedIn,
}) {
  const id = uniqueId('report-text');
  let model = null;
  let rows = [];
  let original = new Map();
  let originalOf = null;
  let titles = new Map();
  let shown = null;
  let active = null;
  let picked = null;
  let editor = null;
  let editing = false;

  const size = h('span', { class: 'more muted' });
  const lead = h('div', { class: 'rp-lead', role: 'status' });
  const marks = h('p', { id: `${id}-marks`, class: 'small muted' });
  const waiting = h('p', { class: 'small', role: 'status' });
  const text = h('pre', { class: 'log rp-text', tabindex: '0', role: 'region', 'aria-describedby': `${id}-marks` });
  const words = h('button', { class: 'btn small rp-words', type: 'button', hidden: true });
  const cuts = h('p', { class: 'small', role: 'status' });
  const edit = h('button', { class: 'btn', type: 'button' }, 'Edit your answers');
  const undo = h('button', { class: 'btn', type: 'button', onclick: onUndo }, 'Undo all my changes');
  const again = h('button', { class: 'btn', type: 'button', onclick: onAgain }, 'Draft again');
  const frame = h('div', { class: 'rp-box' }, text, words);

  // ---- the answers, written again in place ----

  function endEdit() {
    editing = false;
    if (editor) editor.element.hidden = true;
    text.hidden = false;
    edit.disabled = model?.drafting === true;
    edit.focus();
  }

  edit.addEventListener('click', async () => {
    editor ??= (await import('./answers.js')).makeAnswers({
      onDone: async (written) => {
        if (await onAnswers(written)) endEdit();
      },
      onCancel: endEdit,
    });
    if (!editor.element.isConnected) frame.append(editor.element);
    editing = true;
    text.hidden = true;
    words.hidden = true;
    edit.disabled = true;
    editor.open(model.answers, model.needsFirst);
  });

  // ---- the rows ----

  /** One line: its words with the marks in them, and Cut line when the system gathered it. */
  function rowNode(row) {
    const pieces = piecesOf(row.text).map((piece) => (piece.mark
      ? h('mark', { class: `mk ${piece.mark}`, title: piece.mark === 'cut' ? 'Cut by you' : 'Taken out' }, piece.text)
      : piece.text));
    return h('span', { class: `rl ${row.kind}`, 'data-n': row.n },
      h('span', { class: 'rl-text' }, pieces),
      row.typed && h('span', { class: 'rl-tag' }, 'Typed by you'),
      row.kind === 'gathered' && h('button', {
        class: 'btn small rl-cut', type: 'button', tabindex: '-1', 'aria-label': `Cut line ${row.at + 1} of ${titles.get(row.section)}`,
      }, 'Cut line'));
  }

  /** Makes `row` the line that Tab reaches; with `focus`, moves focus to its button. */
  function activate(row, focus = false) {
    active?.classList.remove('act');
    active?.querySelector('.rl-cut').setAttribute('tabindex', '-1');
    active = row;
    if (!row) return;
    row.classList.add('act');
    const button = row.querySelector('.rl-cut');
    button.setAttribute('tabindex', '0');
    row.scrollIntoView?.({ block: 'nearest' });
    if (focus) button.focus();
  }

  function draw() {
    rows = rowsOf(model.now);
    if (originalOf !== model.first) {
      originalOf = model.first;
      original = gatheredOf(model.first);
    }
    titles = new Map(model.now.sections.map((place) => [place.id, place.title]));
    active = null;
    picked = null;
    words.hidden = true;
    fill(text, rows.map(rowNode));
    activate(text.querySelector('.rl.gathered'));
  }

  /** Asks for the text with `next` cut, and leaves focus in the text. */
  async function cut(next) {
    picked = null;
    words.hidden = true;
    await onCut(next);
    getSelection()?.removeAllRanges();
    text.focus();
  }

  text.addEventListener('click', (event) => {
    const button = event.target.closest('.rl-cut');
    if (!button || model.drafting) return;
    const row = rows[Number(button.closest('.rl').getAttribute('data-n'))];
    cut(withLines(model.cuts, row.section, drafted(model.cuts, row.section, row.at)));
  });
  text.addEventListener('focusin', (event) => {
    const row = event.target.closest('.rl.gathered');
    if (row && row !== active) activate(row);
  });
  text.addEventListener('keydown', (event) => {
    if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
    const all = text.querySelectorAll('.rl.gathered');
    if (all.length === 0) return;
    event.preventDefault();
    const next = all.indexOf(active) + (event.key === 'ArrowDown' ? 1 : -1);
    activate(all[Math.min(all.length - 1, Math.max(0, next))], event.target !== text);
  });

  // ---- what is selected ----

  /** What `found`, the selection, would cut: `{kind, cut}`, or null. A whole line selected is a line. */
  function pickOf(found) {
    const place = { cuts: model.cuts, original };
    const [first] = found.covered;
    if (found.covered.length === 1 && !(first.from === 0 && first.to >= rows[first.n].text.length)) {
      const one = wordsPick(place, rows[first.n], first.from, first.to);
      return one && { kind: 'words', cut: one };
    }
    const lines = linesPick(place, found.covered.map(({ n }) => rows[n]));
    return lines && { kind: 'lines', cut: lines };
  }

  document.addEventListener('selectionchange', () => {
    if (model?.mode !== 'ready' || editing) return;
    const found = readSelection(text, frame);
    const pick = found ? pickOf(found) : null;
    // A press on the button may collapse the selection before its click: it keeps what it was made for while it has focus.
    if (pick === null && document.activeElement === words) return;
    picked = pick;
    words.hidden = pick === null;
    if (pick === null) return;
    words.textContent = pick.kind === 'words' ? 'Cut these words' : 'Cut these lines';
    const room = Number.isFinite(frame.clientWidth) ? frame.clientWidth - 150 : found.left;
    words.style.top = `${Math.round(found.top + 4)}px`;
    words.style.left = `${Math.round(Math.max(0, Math.min(found.left, room)))}px`;
  }, { signal: view.signal });
  words.addEventListener('mousedown', (event) => event.preventDefault());
  words.addEventListener('click', () => {
    if (picked === null || model.drafting) return;
    const { kind, cut: one } = picked;
    cut(kind === 'words' ? withWords(model.cuts, one.section, one.line, one.from, one.to) : withLines(model.cuts, one.section, one.line, one.through));
  });

  const live = h('div', { class: 'rp-live', hidden: true },
    marks, waiting, frame, cuts,
    h('div', { class: 'btns' }, edit, undo, again),
    h('p', { class: 'small muted' }, 'The first lines say where the report comes from and are not cut.'));
  const element = h('section', { class: 'card', 'aria-labelledby': `${id}-h` }, h('h2', { id: `${id}-h` }, 'Exactly what is sent', size), lead, live);

  return {
    element,

    /**
     * Draws the card for `next`: `{mode, drafting, first, now, cuts, edited,
     * answers, needsFirst}`. `mode` is `ready` when there is a text, else
     * what is said in its place: `choose`, `drafting`, or another, whose
     * words the page gives as `none`, a node.
     */
    paint(next, none = null) {
      model = next;
      const ready = next.mode === 'ready';
      live.hidden = !ready;
      fill(lead, !ready && (none ?? h('p', { class: 'note' }, WHEN_NONE[next.mode])));
      size.textContent = ready ? holdsWords(next.now) : '';
      if (!ready) {
        if (editing) endEdit();
        return;
      }
      waiting.textContent = next.drafting ? 'Drafting…' : '';
      text.setAttribute('aria-busy', String(next.drafting));
      text.setAttribute('aria-label', textLabel(next.now));
      if (shown !== next.now) {
        shown = next.now;
        draw();
      }
      marks.textContent = marksWords(next.now.cut_by, next.now.cut_by?.you ?? 0);
      cuts.textContent = youCutWords(next.now.cut_by_you);
      undo.hidden = next.cuts.length === 0 && !next.edited;
      edit.disabled = next.drafting || editing;
      undo.disabled = next.drafting;
      again.disabled = next.drafting;
    },
  };
}
