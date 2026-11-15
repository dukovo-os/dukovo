// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// One disk (design 4.3): what it is for, how old it is, what its health
// counts say, what its last scrub fixed, and the buttons: Check now, which
// reads the disk and shows every number under the rows; Scrub the mirror or
// Check every block, a task whose button says how far it is; Replace this
// disk…, which gives the steps and runs nothing; and Grow into it, when the
// disk has room after its last partition. A reader sees no button. The rows
// are drawn again only when what they say changes, and never while an
// action runs, so that a button that was pressed is not replaced under the
// person's hand.

import { fill, h, icon, uniqueId } from '../../dom.js';
import { healthWords } from '../../lib/disk-model.js';
import { attributeRows, detailOf, healthOf } from './disk-detail.js';
import { findDisk } from './model.js';

/** What the shell says on the way, for each action of the buttons. */
const checkWords = (name) => ({ unlock: `check ${name} now`, running: `Checking ${name}`, busy: 'Checking…' });
const growWords = (name) => ({
  unlock: `grow ${name} into its free space`, done: `${name} has grown into its free space.`, unchanged: `${name} has no room to grow into.`, busy: 'Growing…',
});

/** The rows of a panel as one list of terms and what they say. */
function rowsList(rows, hardware) {
  return h('dl', { class: 'rows sto-rows' }, rows.map((row) => [
    h('dt', {}, row.label),
    h('dd', { class: row.tone ?? null },
      row.tone && [icon('warn'), h('span', { class: 'sr-only' }, 'Worth a look: ')],
      row.items ? h('ul', { class: 'sto-items' }, row.items.map((item) => h('li', {}, item))) : row.value,
      row.hardware && hardware && [' ', h('a', { href: '/hardware' }, 'Run one on the Hardware page')]),
  ]));
}

/** The table of a SATA disk's attributes, in a box that scrolls on its own. */
function attributesTable(checked) {
  const rows = attributeRows(checked);
  if (rows.length === 0) return null;
  const id = uniqueId('sto-attrs');
  return h('div', { class: 'tbl-box', role: 'region', tabindex: '0', 'aria-labelledby': id },
    h('table', { class: 'tbl sto-attrs' },
      h('caption', { id, class: 'sr-only' }, `The attributes of ${checked.disk} as it reports them`),
      h('thead', {}, h('tr', {}, ['Attribute', 'Value', 'Worst', 'Threshold', 'Raw', 'Meaning'].map((words, at) => h('th', { scope: 'col', class: at > 0 && at < 5 ? 'num' : '' }, words)))),
      h('tbody', {}, rows.map((row) => h('tr', {}, row.map((cell, at) => h(at === 0 ? 'th' : 'td', at === 0 ? { scope: 'row' } : { class: at < 5 ? 'num' : '' }, cell)))))));
}

/**
 * The panel of the disk `name`. `scrubber` is scrub.js's; `steps(kind,
 * detail)` opens the steps to replace a disk; `reread()` reads the disks
 * again after an action; `links` says which pages exist. `set(answer)` draws
 * the disk as `answer` (`storage show`) has it. Gives `{element, set, check,
 * heading}`; `check()` runs Check now as if it was pressed.
 */
export function makeDisk(view, {
  name, scrubber, steps, reread, links,
}) {
  const state = h('span', { class: 'st off' });
  const more = h('a', { class: 'more', href: '/hardware', hidden: !links.hardware }, 'Temperatures on the Hardware page');
  const heading = h('h2', { id: 'sto-disk-h', tabindex: '-1' }, name, state, more);
  const note = h('p', { class: 'small muted' });
  const rowsBox = h('div', { class: 'grid sto-grid' });
  const controls = h('div', { class: 'sto-controls' });
  const errors = h('div', { class: 'sto-errors' });
  const result = h('div', { class: 'sto-check', hidden: true });
  const said = h('span', { class: 'sr-only', role: 'status' });
  const element = h('section', { class: 'card sto-disk-card', id: 'disk', 'aria-labelledby': 'sto-disk-h' }, heading, note, rowsBox, errors, controls, result, said);

  let answer = null;
  let checked = null;
  let last = null;
  let working = false;

  /** Draws what Check now read: the verdict, the reasons the disk gave and the numbers. */
  function drawResult() {
    if (checked === null) return;
    const verdict = healthWords(checked);
    const id = uniqueId('sto-numbers');
    const raw = h('pre', { id, class: 'cmdtext', tabindex: '0', hidden: true }, JSON.stringify(checked, null, 2));
    const toggle = h('button', { class: 'btn link small', type: 'button', 'aria-expanded': 'false', 'aria-controls': id }, 'Show the numbers as diskhealth prints them');
    toggle.addEventListener('click', () => {
      const open = toggle.getAttribute('aria-expanded') !== 'true';
      toggle.setAttribute('aria-expanded', String(open));
      toggle.textContent = open ? 'Hide the numbers as diskhealth prints them' : 'Show the numbers as diskhealth prints them';
      raw.hidden = !open;
    });
    const reasons = Array.isArray(checked.reasons) ? checked.reasons : [];
    const none = !attributeRows(checked).length;
    result.hidden = false;
    fill(result,
      h('h3', { tabindex: '-1' }, `What ${name} reports now`),
      h('p', {}, h('span', { class: `st ${verdict.tone}` }, verdict.word), ' ', verdict.note),
      reasons.length > 0 && h('ul', { class: 'sto-items' }, reasons.map((reason) => h('li', {}, reason))),
      none && h('p', { class: 'small muted' }, 'This drive reports a health log, not a list of attributes: its numbers are the rows above.'),
      attributesTable(checked),
      toggle, raw);
    said.textContent = `${name} checked: ${verdict.word}.`;
  }

  /** Reads the disk's health now. */
  async function check(button = controls.querySelector('[data-action="check"]')) {
    working = true;
    view.slot.replaceChildren();
    errors.replaceChildren();
    try {
      const done = await view.api.run('storage.health', { disk: name }, { words: checkWords(name), button, where: { slot: errors } });
      if (done.ok) {
        checked = healthOf(done.result, name);
        drawResult();
        result.querySelector('h3')?.focus();
      }
    } catch (error) {
      view.problem(error, { slot: errors });
    } finally {
      working = false;
    }
    draw(true);
  }

  /** Grows the disk's last partition and its filesystem. */
  async function grow(button) {
    working = true;
    errors.replaceChildren();
    let done = { ok: false };
    try {
      done = await view.api.run('storage.grow', { disk: name }, { words: growWords(name), button, where: { slot: errors } });
    } catch (error) {
      view.problem(error, { slot: errors });
    } finally {
      working = false;
    }
    if (done.ok) reread();
    else draw(true);
  }

  /** The buttons of `detail` for an administrator. */
  function buttonsOf(detail) {
    const buttons = [];
    if (detail.hasHealth) {
      const button = h('button', { class: 'btn', type: 'button', 'data-action': 'check' }, 'Check now');
      button.addEventListener('click', () => check(button));
      buttons.push(h('div', { class: 'sto-action' }, button, view.api.inline('storage.health', { disk: name })));
    }
    if (detail.scrub) buttons.push(scrubber.button(detail.scrub.mount, detail.scrub.label));
    if (detail.mirror) {
      const replace = h('button', { class: 'btn', type: 'button', 'data-action': 'replace' }, 'Replace this disk…');
      replace.addEventListener('click', () => steps('replace', { disk: name, answer }));
      buttons.push(h('div', { class: 'sto-action' }, replace));
    }
    return buttons;
  }

  /** Draws the rows and the buttons, when what they say is not what is drawn. */
  function draw(force = false) {
    if (answer === null) return;
    if (working && !force) return;
    const disk = findDisk(answer, name);
    if (disk === null) return;
    const admin = view.session.admin === true;
    const detail = detailOf(disk, answer, { time: view.time, now: Math.floor(Date.now() / 1000), checked, admin });
    const key = JSON.stringify([detail, admin, links]);
    state.className = `st ${detail.health.tone}`;
    state.textContent = detail.health.word;
    note.textContent = detail.line;
    if (key === last) return;
    last = key;
    const held = element.contains(document.activeElement) ? document.activeElement.dataset?.action ?? null : null;
    const half = Math.ceil(detail.rows.length / 2);
    fill(rowsBox, rowsList(detail.rows.slice(0, half), links.hardware), rowsList(detail.rows.slice(half), links.hardware));
    if (!admin) {
      fill(controls);
    } else {
      fill(controls,
        h('div', { class: 'btns sto-buttons' }, buttonsOf(detail)),
        detail.scrub && h('p', { class: 'hint' }, detail.scrub.hint),
        detail.grow && h('div', { class: 'sto-grow' }, h('p', {}, detail.grow.words), growButton()));
    }
    if (held) controls.querySelector(`[data-action="${held}"]`)?.focus();
    scrubber.paint();
  }

  /** The button that grows the disk, with its command under it when Show commands is on. */
  function growButton() {
    const button = h('button', { class: 'btn', type: 'button', 'data-action': 'grow' }, 'Grow into it');
    button.addEventListener('click', () => grow(button));
    return h('div', { class: 'sto-action' }, button, view.api.inline('storage.grow', { disk: name }));
  }

  return {
    element,
    heading,
    check,
    set(next) {
      answer = next;
      draw();
    },
  };
}
