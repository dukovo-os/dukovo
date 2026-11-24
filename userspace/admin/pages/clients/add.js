// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Add a client: its name, how it connects, what it may reach, area by
// area, and when it ends. Adding is the administrator's approval: authd
// takes the unlocked console as the proof that a terminal gives with the
// password, and nothing connects before. Read when the button is pressed.

import {
  busy, fill, h, icon, uniqueId,
} from '../../dom.js';
import { head, openModal } from '../../shell/dialogs.js';
import { askToUnlock } from '../../unlock.js';
import {
  addCommand, chosenFor, DAYS, DAYS_DEFAULT, endsWords, levelsOf, nameProblem, scopesOf,
} from './model.js';
import { commandBox, refuse } from './parts.js';

/** The route that adds a client of the person's own, which is not an action: authd keeps them. */
const ADD = '/api/1/account/clients/add';

/** The starts the dialog offers, and the one it opens with: reading alone. */
const STARTS = [['read', 'Read everything'], ['all', 'Read all and ask, everywhere'], ['choose', 'Choose']];

const LEVELS = [
  h('b', {}, 'Read'), ' is what your account sees before you unlock. ', h('b', {}, 'Read all'),
  ' adds what only administrators see, such as the whole log. ', h('b', {}, 'Ask'),
  ' lets it ask for changes there, and each one waits for a yes.',
];
const WHO_GETS_IN = ' These decide who gets in, so a client can read them and never ask to change them. Passwords, keys and tokens are never shown to a client anywhere.';
const NO_TOKEN = 'The server added the client and gave no token for it. Remove it, and add it again.';

/** A pressed-or-not button of a row of choices. */
function choice(words, press) {
  return h('button', { type: 'button', 'aria-pressed': 'false', onclick: press }, words);
}

/** Marks which of `buttons`, `[id, button]`, is the one pressed. */
function mark(buttons, pressed) {
  for (const [id, button] of buttons) {
    button.setAttribute('aria-pressed', String(id === pressed));
    button.classList.toggle('on', id === pressed);
  }
}

/**
 * Opens the dialog and resolves, when it closes, with the client that was
 * added, `{name, way, token}`, where `token` is null for one that connects
 * over SSH, or with null when none was. `areas` are `GET
 * /api/1/clients/areas`'s, and `taken` the names of the person's clients.
 */
export async function openAdd(view, { areas, taken }) {
  const id = uniqueId('cl-add');
  let start = 'read';
  let chosen = chosenFor(areas, start);
  let added = null;

  const name = h('input', {
    id: `${id}-name`, class: 'input mono', type: 'text', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off', maxlength: '32',
    'aria-describedby': `${id}-name-hint`, autofocus: true,
  });
  const ssh = h('input', { type: 'radio', name: `${id}-way`, value: 'ssh', checked: true });
  const https = h('input', { type: 'radio', name: `${id}-way`, value: 'https' });
  const ways = [
    [ssh, h('label', { class: 'radio-card on' }, ssh, h('span', {}, h('b', {}, 'Over SSH'), ' ', h('span', { class: 'pill ok' }, 'Safest'), h('br'),
      h('span', { class: 'muted' }, 'The AI client starts ', h('span', { class: 'mono' }, 'admin mcp'), ' on this server through your own SSH sign-in. There is no token to keep on the other machine, and no certificate to trust.')))],
    [https, h('label', { class: 'radio-card' }, https, h('span', {}, h('b', {}, 'Over HTTPS, with a token'), h('br'),
      h('span', { class: 'muted' }, 'For an AI client that cannot use SSH. It gets a token, shown once. Keep it as you keep a password.')))],
  ];
  const ends = h('select', { id: `${id}-ends`, class: 'input', 'aria-describedby': `${id}-ends-hint` },
    DAYS.map((days) => h('option', { value: String(days), selected: days === DAYS_DEFAULT }, endsWords(days, view.time.zone))));

  const asked = () => ({
    name: name.value.trim(),
    way: https.checked ? 'https' : 'ssh',
    scopes: scopesOf(areas, chosen),
    days: Number(ends.value) || DAYS_DEFAULT,
  });
  const command = view.prefs.commands === 'shown'
    ? commandBox('The same at a terminal, which asks for your password', addCommand(asked()), 'The command that adds this client')
    : null;

  const starts = STARTS.map(([which, words]) => [which, choice(words, () => {
    start = which;
    if (which !== 'choose') chosen = chosenFor(areas, which);
    draw();
  })]);
  const rows = areas.map((area) => {
    const buttons = levelsOf(area).map((level) => [level.id, choice(level.label, () => {
      start = 'choose';
      chosen = { ...chosen, [area.area]: level.id };
      draw();
    })]);
    return {
      area,
      buttons,
      element: h('div', { class: 'area' },
        h('span', {}, area.who_gets_in && [icon('lock'), ' '], area.title),
        h('span', { class: 'seg', role: 'group', 'aria-label': area.title }, buttons.map(([, button]) => button))),
    };
  });

  function draw() {
    mark(starts, start);
    for (const row of rows) mark(row.buttons, chosen[row.area.area]);
    for (const [input, label] of ways) label.classList.toggle('on', input.checked);
    command?.set(addCommand(asked()));
  }

  const errors = h('div', {});
  const confirm = h('button', { class: 'btn default', type: 'submit' }, 'Add the client');
  const cancel = h('button', { class: 'btn', type: 'button' }, 'Cancel');
  const half = Math.ceil(rows.length / 2);
  const form = h('form', { method: 'dialog', novalidate: true },
    head(`${id}-h`, 'plus', 'Add an AI client'),
    h('div', { class: 'dialog-body' },
      h('div', { class: 'field cl-short' },
        h('label', { for: name.id }, 'Name'), name,
        h('span', { id: `${id}-name-hint`, class: 'hint' }, 'Letters, digits and hyphens. The log and the journal show it.')),
      h('fieldset', { class: 'choices' }, h('legend', {}, 'How it connects'), ways.map(([, label]) => label)),
      h('fieldset', { class: 'choices' }, h('legend', {}, 'What it may reach'),
        h('div', { class: 'btns' }, h('span', { class: 'seg', role: 'group', 'aria-label': 'Start from' }, starts.map(([, button]) => button))),
        h('p', { class: 'small muted' }, LEVELS),
        h('div', { class: 'areas' },
          h('div', {}, rows.slice(0, half).map((row) => row.element)),
          h('div', {}, rows.slice(half).map((row) => row.element))),
        h('p', { class: 'small muted' }, icon('lock'), WHO_GETS_IN)),
      h('div', { class: 'field cl-short' },
        h('label', { for: ends.id }, 'Ends'), ends,
        h('span', { id: `${id}-ends-hint`, class: 'hint' }, 'A client always ends. A year at most.')),
      errors,
      command?.element),
    h('div', { class: 'dialog-foot' },
      h('span', { class: 'small muted cl-approval' }, `Adding it is your approval, as ${view.session.name}. Nothing connects until you add it.`),
      cancel, confirm));
  const dialog = h('dialog', { class: 'dialog cl-add', 'aria-labelledby': `${id}-h` }, form);

  form.addEventListener('input', draw);
  form.addEventListener('change', draw);
  cancel.addEventListener('click', () => dialog.close(''));
  form.addEventListener('submit', async (event) => {
    event.preventDefault();
    fill(errors);
    const call = asked();
    const problem = nameProblem(call.name, taken);
    if (problem) {
      refuse(errors, problem, name);
      return;
    }
    if (call.scopes.length === 0) {
      refuse(errors, 'Choose at least one area for it to reach.');
      return;
    }
    if (!(await askToUnlock(view.session, `add the AI client ${call.name}`))) return;
    await busy([confirm], async () => {
      try {
        const answer = await view.api.post(ADD, { proof: '', ...call });
        const token = typeof answer?.token === 'string' ? answer.token : null;
        if (call.way === 'https' && token === null) {
          refuse(errors, NO_TOKEN);
          return;
        }
        added = { name: call.name, way: call.way, token };
        dialog.close('done');
      } catch (error) {
        view.problem(error, { slot: errors });
      }
    });
  });
  view.signal.addEventListener('abort', () => dialog.close(''), { once: true });
  draw();
  await openModal(dialog);
  return added;
}
