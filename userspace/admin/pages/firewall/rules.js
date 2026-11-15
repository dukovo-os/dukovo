// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Rules card of the Firewall page (design 10.3): the rules in the order
// they are checked, each in words with its command while commands are shown,
// Change and Remove for an administrator, the lines of the file that are no
// rule, and the machine's rule for what no rule decides. The table is drawn
// again only when what it says changes, so that a button in hand stays in hand.

import { fill, h } from '../../dom.js';
import { problemWords, ruleRows } from './report-model.js';
import {
  argsOfRule, canChange, DEFAULT_WORDS, ruleOf,
} from './rules-model.js';

const COLUMNS = ['#', 'Rule', ''];

/** The words under the heading while a trial runs. */
export const TRIAL_WORDS = 'Keep or undo the change being tried first.';

/** The rows of grey bars that stand for the table while it is read. */
const SKELETON_ROWS = 3;

/**
 * The card. `onChange(number)` and `onRemove(number)` are called when
 * Change or Remove is pressed, and `onAdd()` when Add a rule is. `set(listed,
 * {admin, trial})` draws `firewall list --json`'s answer.
 */
export function makeRules(view, { onChange, onRemove, onAdd }) {
  const head = h('h2', { id: 'fw-rules-h' }, 'Rules');
  const add = h('button', { class: 'btn small', type: 'button', id: 'fw-add-open' }, 'Add a rule');
  add.addEventListener('click', onAdd);
  const body = h('tbody');
  const caption = h('caption', { class: 'sr-only' }, 'Rules, in the order they are checked');
  const table = h('table', { class: 'tbl stack fw-tbl', 'aria-busy': 'true' }, caption,
    h('thead', {}, h('tr', {}, ...COLUMNS.map((name) => h('th', { scope: 'col', class: name === '' ? 'sr-only' : '' }, name === '' ? 'Actions' : name)))),
    body);
  const box = h('div', { class: 'tbl-box' }, table);
  const none = h('p', { class: 'muted fw-none', hidden: true }, 'There are no rules.');
  const wait = h('p', { id: 'fw-rules-wait', class: 'note fw-wait', role: 'status' });
  const problems = h('ul', { class: 'fw-lines', 'aria-label': 'Lines of the file that are not rules', hidden: true });
  const fallback = h('p', { class: 'fw-default' }, h('b', {}, 'When no rule matches: '), DEFAULT_WORDS);
  const element = h('section', { class: 'card fw-rules', 'aria-labelledby': head.id },
    head, wait, box, none, problems, fallback, h('div', { class: 'btns fw-rules-btns' }, add));
  const skeleton = () => fill(body, Array.from({ length: SKELETON_ROWS }, () => h('tr', { 'aria-hidden': 'true' },
    h('th', { scope: 'row' }, h('span', { class: 'fw-bar' })), h('td', {}, h('span', { class: 'fw-bar wide' })), h('td', {}))));
  skeleton();

  /** A button of a row, which is off while a trial runs. */
  function button(words, label, number, press, trial, danger = false) {
    const one = h('button', {
      class: `btn small${danger ? ' danger' : ''}`, type: 'button', 'aria-label': `${words} for rule ${number}`, 'data-action': label.toLowerCase(),
      'aria-disabled': trial ? 'true' : null, 'aria-describedby': trial ? wait.id : null,
    }, label);
    one.addEventListener('click', () => {
      if (one.getAttribute('aria-disabled') !== 'true') press(number);
    });
    return one;
  }

  /** A row of the table, from `rows`' entry, with the buttons an administrator has. */
  function rowOf(row, { admin, trial }) {
    const rule = ruleOf(listedByNumber.get(row.number));
    const buttons = [];
    if (admin && canChange(rule)) {
      buttons.push(button('Change', 'Change', row.number, onChange, trial), button('Remove', 'Remove', row.number, onRemove, trial, true));
    } else if (admin) {
      buttons.push(h('span', { class: 'small muted' }, 'Changed at a terminal'));
    }
    return h('tr', { 'data-number': String(row.number) },
      h('th', { scope: 'row', 'data-label': COLUMNS[0] }, String(row.number)),
      h('td', { 'data-label': COLUMNS[1] }, h('span', { class: 'fw-words' }, row.words), view.api.inline('firewall.add', argsOfRule(rule))),
      h('td', { class: 'end fw-buttons', 'data-label': '' }, ...buttons));
  }

  let listedByNumber = new Map();
  let last = null;
  return {
    element,
    /** Draws the rules of `listed` for `{admin, trial}`. */
    set(listed, { admin, trial }) {
      listedByNumber = new Map((listed.rules ?? []).map((entry) => [entry.number, entry]));
      const rows = ruleRows(listed);
      const lines = problemWords(listed);
      add.hidden = !admin;
      add.setAttribute('aria-disabled', String(trial));
      if (trial) add.setAttribute('aria-describedby', wait.id); else add.removeAttribute('aria-describedby');
      wait.textContent = trial ? TRIAL_WORDS : '';
      const key = JSON.stringify([rows, lines, admin, trial]);
      table.removeAttribute('aria-busy');
      box.hidden = rows.length === 0;
      none.hidden = rows.length > 0;
      problems.hidden = lines.length === 0;
      if (key === last) return;
      last = key;
      const held = body.contains(document.activeElement) ? [document.activeElement.closest('tr')?.dataset.number, document.activeElement.dataset.action] : null;
      fill(body, rows.map((row) => rowOf(row, { admin, trial })));
      fill(problems, lines.map((line) => h('li', {}, line.words, ' ', h('code', { class: 'mono' }, line.text))));
      if (held?.[0]) body.querySelector(`tr[data-number="${held[0]}"] [data-action="${held[1]}"]`)?.focus();
    },
  };
}
