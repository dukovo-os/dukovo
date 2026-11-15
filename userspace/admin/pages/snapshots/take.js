// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Take a snapshot now (design 5.5), for an administrator: each place a
// checkbox, all on, and an optional name. The action makes the snapshot of
// one place, so the dialog runs it once per place in order and says each
// place's result; a place that fails is told by the shell's own words and
// does not stop the others. The page reads this file when the button is
// first pressed.

import {
  busy, clearError, errorSlot, fill, h, showError, uniqueId,
} from '../../dom.js';
import { inOrder, placeName } from '../../lib/snapshot-model.js';
import { head, openModal } from '../../shell/dialogs.js';
import { snapshotNameProblem, takenWords } from './form-model.js';

/**
 * Opens the dialog for `paths`, the places that `snap list` names. `changed()`
 * is called when a snapshot was made, so that the page reads again. Resolves
 * when the dialog closes.
 */
export function openTake(view, { paths, changed }) {
  const id = uniqueId('snap-take');
  const headingId = `${id}-h`;
  const places = inOrder(paths);
  const boxes = places.map((path) => h('input', { type: 'checkbox', checked: true, value: path, id: `${id}-${path.replace(/[^a-z0-9]+/gi, '-')}` }));
  const name = h('input', { id: `${id}-name`, class: 'input', type: 'text', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off', 'aria-describedby': `${id}-name-hint` });
  const nameSlot = errorSlot(`${id}-name-error`);
  const errors = h('div', { class: 'snap-errors' });
  const result = h('div', { class: 'snap-result', role: 'status' });
  const take = h('button', { class: 'btn default', type: 'submit' }, 'Take a snapshot');
  const close = h('button', { class: 'btn', type: 'button' }, 'Cancel');

  const chosen = () => places.filter((_, at) => boxes[at].checked);
  const argsOf = (place) => (name.value.trim() === '' ? { place } : { place, name: name.value.trim() });

  const command = h('div', { class: 'snap-command' });
  let timer = null;
  const showCommand = () => {
    clearTimeout(timer);
    timer = setTimeout(() => {
      const [first] = chosen();
      fill(command, ...(first && snapshotNameProblem(name.value.trim()) === '' ? [h('h3', {}, 'This runs, once for each place'), view.api.panel('snapshots.take', argsOf(first))] : []));
    }, 300);
  };
  view.signal.addEventListener('abort', () => clearTimeout(timer), { once: true });
  for (const box of boxes) box.addEventListener('change', showCommand);
  name.addEventListener('input', () => {
    clearError(nameSlot, name);
    showCommand();
  });

  const dialog = h('dialog', { class: 'dialog', 'aria-labelledby': headingId },
    h('form', { method: 'dialog', novalidate: true },
      head(headingId, 'plus', 'Take a snapshot now'),
      h('div', { class: 'dialog-body' },
        h('p', {}, 'A snapshot keeps a place as it is now, read-only. It takes room only for what changes after it.'),
        h('fieldset', { class: 'choices' }, h('legend', {}, 'Places'),
          places.map((path, at) => h('label', { class: 'check', for: boxes[at].id }, boxes[at], h('span', {}, `${placeName(path)} `, h('span', { class: 'muted' }, path))))),
        h('div', { class: 'field' }, h('label', { for: `${id}-name` }, 'Name (optional)'), name,
          h('span', { id: `${id}-name-hint`, class: 'hint' }, 'A snapshot with a name is kept until someone deletes it, so give one only for a reason, such as before-the-move.'), nameSlot),
        errors, result, command),
      h('div', { class: 'dialog-foot' }, close, take)));

  let working = false;
  let made = false;
  close.addEventListener('click', () => dialog.close('close'));
  dialog.addEventListener('cancel', (event) => {
    if (working) event.preventDefault();
  });
  dialog.querySelector('form').addEventListener('submit', async (event) => {
    event.preventDefault();
    clearError(nameSlot, name);
    result.replaceChildren();
    const found = snapshotNameProblem(name.value.trim());
    if (found) {
      showError(nameSlot, { title: found }, name);
      return;
    }
    const picked = chosen();
    if (picked.length === 0) {
      result.textContent = 'Choose at least one place.';
      return;
    }
    working = true;
    errors.replaceChildren();
    const results = [];
    await busy([close], async () => {
      for (const place of picked) {
        const done = await view.api.run('snapshots.take', argsOf(place), {
          words: { unlock: 'take a snapshot', done: `Made a snapshot of ${placeName(place)}.`, busy: 'Taking…' }, button: take, where: { slot: errors },
        }).catch(() => ({ ok: false }));
        results.push({ place, ok: done.ok === true });
        // A person who declined to unlock has declined for the rest too.
        if (done.cancelled) break;
      }
    });
    working = false;
    if (results.some((one) => one.ok)) {
      made = true;
      changed();
    }
    fill(result, ...takenWords(results).map((words) => h('p', {}, words)));
    close.textContent = 'Close';
  });
  return openModal(dialog).then(() => made);
}
