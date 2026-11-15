// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The palette's run view (design 11.4): an action's form, filled from the
// thing it was found with, the command it makes under the form as a
// one-line `inline-cmd`, and Run. Enter in the form runs it, through
// lib/action.js, so a locked administrator gets the unlock dialog first and
// admind's own words land under the field they are about. Tab on a row opens
// the view with the Show command panel under the form.
//
// The view is for a change and for a read that still needs arguments; a read
// whose arguments are all there is run from the list and goes to the result
// view. A reader sees ADM-122's words and no Run button.

import { h, icon, uniqueId } from '../dom.js';
import { needsAdministrator, readerProblem } from '../lib/action-model.js';
import { schemaForm } from '../lib/schema-form.js';
import { formFor } from '../lib/schema-model.js';
import { showKey } from '../lib/command-model.js';
import { runWords } from './index-model.js';

// How long the command waits for the person to stop typing before it is read again.
const COMMAND_MS = 250;

/** The head of a view: Back, and the heading that takes focus when the view opens. */
export function viewHead({ title, onBack, pills = [] }) {
  const heading = h('h2', { id: uniqueId('pal-title'), tabindex: '-1' }, title);
  return {
    heading,
    element: h('div', { class: 'pal-head' },
      h('button', { class: 'btn small', type: 'button', onclick: onBack }, 'Back'),
      heading,
      h('span', { class: 'spacer' }),
      ...pills.map((pill) => h('span', { class: `pill ${pill.tone}`.trim() }, pill.label))),
  };
}

/**
 * The view for `row` and its action `entry` (from `GET /api/1/actions`;
 * undefined when this server does not list it). `deps`: `session()`,
 * `post` and `run` (the shell's), `commands` (the shell's Show command),
 * `pills` for the row, `panel` (whether the panel is open from the start),
 * `onBack()` and `onDone({ok, result, task, args})`. Gives `{element,
 * focus, dispose}`.
 */
export function runView(row, entry, deps) {
  const { session, post, run, commands, pills, onBack, onDone } = deps;
  const head = viewHead({ title: row.title, onBack, pills });
  const controller = new AbortController();
  if (!entry) {
    return {
      element: [head.element, h('div', { class: 'pal-pane' }, h('p', { class: 'msg' }, icon('warn'),
        h('span', {}, `This server does not have the action ${row.target.action}. It may come with a later update.`)))],
      focus: () => head.heading.focus(),
      dispose: () => controller.abort(),
    };
  }

  const form = schemaForm(formFor(entry.input), { args: row.target.args, onChange: () => preview() });
  const reader = needsAdministrator(entry) && !session().admin;
  const slot = h('div', { class: 'pal-slot' });
  const runButton = reader ? null : h('button', { class: 'btn default', type: 'submit' }, entry.changes ? 'Run' : 'Show the answer');

  // The command, one line, which opens the panel under it.
  let open = deps.panel === true;
  let token = 0;
  let timer = null;
  let panelFor = null;
  const lines = new Map();
  const line = h('button', { class: 'inline-cmd', type: 'button', hidden: true, 'aria-expanded': String(open) });
  const holder = h('div', { class: 'pal-panel', hidden: !open });
  const note = h('p', { class: 'small muted', hidden: true }, 'Fill in the form to see the command.');
  const panelId = uniqueId('pal-panel');
  holder.id = panelId;
  if (commands?.panel) line.setAttribute('aria-controls', panelId);
  else line.removeAttribute('aria-expanded');

  function showPanel(args) {
    const key = args ? showKey(entry.id, args) : null;
    note.hidden = !open || args !== null;
    holder.hidden = !open || args === null;
    if (!open || args === null || !commands?.panel || key === panelFor) return;
    panelFor = key;
    holder.replaceChildren(commands.panel(entry.id, args, { signal: controller.signal }));
  }

  async function preview() {
    clearTimeout(timer);
    timer = setTimeout(async () => {
      const mine = (token += 1);
      const { args, problems } = form.read();
      if (args === null || problems.length > 0) {
        line.hidden = true;
        panelFor = null;
        showPanel(null);
        return;
      }
      const key = showKey(entry.id, args);
      try {
        if (!lines.has(key)) lines.set(key, post('/api/1/show', { action: entry.id, args }, { signal: controller.signal }).then((answer) => answer.command));
        const command = await lines.get(key);
        if (mine !== token) return;
        line.textContent = command;
        line.hidden = false;
        showPanel(args);
      } catch {
        lines.delete(key);
        if (mine === token) line.hidden = true;
      }
    }, COMMAND_MS);
  }

  line.addEventListener('click', () => {
    if (!commands?.panel) return;
    open = !open;
    line.setAttribute('aria-expanded', String(open));
    const { args, problems } = form.read();
    showPanel(args !== null && problems.length === 0 ? args : null);
  });

  async function submit(event) {
    event.preventDefault();
    if (reader) return;
    const { args, problems } = form.read();
    if (args === null || problems.length > 0) {
      form.show(problems);
      return;
    }
    slot.replaceChildren();
    const result = await run(entry.id, args, {
      words: runWords(row.title), button: runButton, where: { slot, fields: form.fields, input: form.input, button: runButton },
    });
    if (result.cancelled || (!result.ok && !result.error)) runButton?.focus();
    else if (result.ok) onDone({ ...result, args });
  }

  const element = [
    head.element,
    h('div', { class: 'pal-pane' },
      reader && bannerFor(session()),
      h('form', { novalidate: true, onsubmit: submit },
        form.element,
        h('div', { class: 'pal-cmd' }, line, note, holder),
        slot,
        runButton && h('div', { class: 'pal-run-actions' }, runButton))),
  ];

  preview();
  return {
    element,
    focus() {
      if (form.input) form.focusFirst();
      else (runButton ?? head.heading).focus();
    },
    dispose() {
      clearTimeout(timer);
      controller.abort();
    },
  };
}

/** The words for an account that may look and not change: ADM-122's, with no button to press. */
function bannerFor(session) {
  const words = readerProblem(session);
  return h('div', { class: 'banner warn', role: 'status' },
    icon('lock'),
    h('div', {}, h('p', {}, h('b', {}, `${words.title}. `), words.detail, ' ', words.fix)));
}
