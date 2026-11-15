// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The log's own space (design 4.3): how much room the system's log takes,
// what it keeps and for how long, Change… for its limits and Trim now. Only
// an administrator reads the size, as logd answers it to root alone: a
// reader is told who does, and an administrator whose console is locked
// is asked to unlock. The card reads when it opens, when the console is
// unlocked or locked, and after a change; never on a timer.

import {
  clearError, errorSlot, fill, h, showError, uniqueId,
} from '../../dom.js';
import { askToUnlock, isUnlocked } from '../../unlock.js';
import {
  formOf, LOG_FIELDS, logArgs, logWords,
} from './log-model.js';

/** The words that say who sees the size, for a reader and for an administrator whose console is locked. */
const OTHERS = Object.freeze({
  reader: 'Administrators see the log\'s size.',
  locked: 'Unlock to see the log\'s size.',
});

/** The least wait before the command under the form is made again, so that typing is not a request for each key. */
const TYPING_MS = 400;

/** The grey bars that stand where the words are read. */
const bars = () => [0, 1].map(() => h('span', { class: 'sto-bar wide' }));

/** Draws the log into `card`, the section that the page holds in its place. Gives `{load}`, which reads the log. */
export function makeLog(view, card) {
  const errors = h('div', { class: 'sto-errors' });
  const body = h('div', { class: 'sto-log-body', 'aria-hidden': 'true' }, bars());
  fill(card, h('h2', { id: 'sto-log-h' }, 'The log\'s own space'), errors, body);

  const modeOf = () => (!view.session.admin ? 'reader' : isUnlocked() ? 'open' : 'locked');
  let mode = modeOf();
  let latest = 0;
  let log = null;
  let editing = false;

  /** The words and the button of a reader or of a console that is locked. */
  function drawClosed() {
    const unlock = h('button', { class: 'btn', type: 'button' }, 'Unlock');
    unlock.addEventListener('click', async () => {
      if (await askToUnlock(view.session, 'see the log\'s size')) recheck();
    });
    fill(body, h('p', { class: 'muted' }, OTHERS[mode]), mode === 'locked' && h('div', { class: 'btns' }, unlock));
    body.removeAttribute('aria-hidden');
  }

  /** Trims the log to its limits now. */
  async function trim(button) {
    errors.replaceChildren();
    const done = await view.api.run('storage.log-trim', {}, {
      words: { unlock: 'trim the log', done: 'The log was trimmed to its limits.', unchanged: 'The log was already within its limits.', busy: 'Trimming…' },
      button, where: { slot: errors },
    }).catch((error) => {
      view.problem(error, { slot: errors });
      return { ok: false };
    });
    if (done.ok) load();
  }

  /** The form of the limits, with the command under it. */
  function formDraw() {
    const inputs = {};
    const slots = {};
    const start = formOf(log.limits);
    const fields = LOG_FIELDS.map((field) => {
      const id = uniqueId('sto-log');
      const input = h('input', { class: 'input', id, name: field.name, type: 'text', value: start[field.name], placeholder: 'no limit', autocomplete: 'off', spellcheck: 'false', 'aria-describedby': `${id}-hint` });
      inputs[field.name] = input;
      slots[field.name] = errorSlot(`${id}-error`);
      return h('div', { class: 'field' },
        h('label', { for: id }, field.label, field.unit && ` (${field.unit})`),
        input, h('span', { id: `${id}-hint`, class: 'hint' }, field.hint), slots[field.name]);
    });
    const command = h('div', { class: 'sto-command' });
    const formErrors = h('div', { class: 'sto-errors' });
    const saved = h('p', { class: 'small muted', role: 'status' });
    const save = h('button', { class: 'btn default', type: 'submit' }, 'Save');
    const cancel = h('button', { class: 'btn', type: 'button' }, 'Cancel');
    const form = h('form', { class: 'sto-form', novalidate: true }, h('div', { class: 'sto-fields' }, fields), formErrors, saved, command, h('div', { class: 'btns' }, save, cancel),
      h('p', { class: 'small muted' }, 'These limits are kept in the file /system/settings/log, which log(5) describes. logtrim(8) trims the log at the terminal.'));

    const values = () => Object.fromEntries(Object.entries(inputs).map(([name, input]) => [name, input.value]));
    let timer = null;
    /** Shows the command for what the form says now, once it is right. */
    function showCommand() {
      const found = logArgs(values(), log.limits);
      command.replaceChildren(...(found.args ? [h('h3', {}, 'This runs'), view.api.panel('storage.log-set', found.args)] : []));
    }
    for (const input of Object.values(inputs)) {
      input.addEventListener('input', () => {
        clearError(slots[input.name], input);
        saved.textContent = '';
        clearTimeout(timer);
        timer = setTimeout(showCommand, TYPING_MS);
      });
    }
    view.signal.addEventListener('abort', () => clearTimeout(timer), { once: true });
    cancel.addEventListener('click', () => {
      editing = false;
      draw();
      body.querySelector('[data-action="change"]')?.focus();
    });
    form.addEventListener('submit', async (event) => {
      event.preventDefault();
      formErrors.replaceChildren();
      const found = logArgs(values(), log.limits);
      if (found.errors) {
        // The first field's words come last, so that focus lands on it.
        for (const field of [...LOG_FIELDS].reverse()) if (found.errors[field.name]) showError(slots[field.name], { title: found.errors[field.name] }, inputs[field.name]);
        return;
      }
      if (found.nothing) {
        saved.textContent = 'Nothing to change.';
        return;
      }
      const done = await view.api.run('storage.log-set', found.args, {
        words: { unlock: 'change the log\'s limits', done: 'The log\'s limits are changed.', unchanged: 'The log\'s limits were already so.', busy: 'Saving…' },
        button: save, where: { slot: formErrors, fields: inputs },
      }).catch((error) => {
        view.problem(error, { slot: formErrors, fields: inputs });
        return { ok: false };
      });
      if (!done.ok) return;
      editing = false;
      await load();
      body.querySelector('[data-action="change"]')?.focus();
    });
    return form;
  }

  /** Draws what the log says, with the buttons for an administrator, or the form when it is open. */
  function draw() {
    const words = logWords(log);
    const change = h('button', { class: 'btn', type: 'button', 'data-action': 'change' }, 'Change…');
    change.addEventListener('click', () => {
      editing = true;
      draw();
      body.querySelector('input')?.focus();
    });
    const trimButton = h('button', { class: 'btn', type: 'button', 'data-action': 'trim' }, 'Trim now');
    trimButton.addEventListener('click', () => trim(trimButton));
    body.removeAttribute('aria-hidden');
    fill(body,
      h('p', {}, words.use),
      h('p', {}, words.keeps),
      words.wrong.length > 0 && h('div', { class: 'note' }, h('p', {}, 'The file of limits has mistakes, which logd leaves out:'), h('ul', { class: 'sto-items' }, words.wrong.map((line) => h('li', { class: 'mono' }, line)))),
      editing ? formDraw() : h('div', { class: 'btns' },
        change,
        h('div', { class: 'sto-action' }, trimButton, view.api.inline('storage.log-trim', {}))));
  }

  /** Reads the log's size and limits. Only the last read counts. */
  async function load() {
    latest += 1;
    const mine = latest;
    mode = modeOf();
    errors.replaceChildren();
    if (mode !== 'open') {
      log = null;
      editing = false;
      drawClosed();
      return;
    }
    const done = await view.api.run('storage.log', {}, { words: { unlock: 'see the log\'s size' }, where: { slot: errors } }).catch((error) => {
      view.problem(error, { slot: errors });
      return { ok: false };
    });
    if (mine !== latest) return;
    if (!done.ok) {
      if (log === null) fill(body, h('p', { class: 'muted' }, 'The log\'s size could not be read.'));
      body.removeAttribute('aria-hidden');
      return;
    }
    log = done.result;
    draw();
  }

  /** Reads again when the console was unlocked or locked since the last read. */
  function recheck() {
    if (modeOf() !== mode) load();
  }
  view.on('unlock', recheck);

  return { load };
}
