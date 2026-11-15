// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Restore files from a backup place (design 5.7): choose a place and a time
// of the backups there, and the backup is received into the place's
// snapshots as restored-TIME, a task. Nothing is overwritten at any step:
// when it ends the page shows that snapshot, where the person picks the
// files, as in any snapshot. The page reads this file when Restore files…
// or Restore this… is first pressed.

import { busy, fill, h, uniqueId } from '../../dom.js';
import { beside } from '../../lib/snapshot-words.js';
import { head, openModal } from '../../shell/dialogs.js';
import { heldBackups } from './detail-model.js';

/**
 * Opens the dialog for the backup place `place` (a row of `backup list`)
 * with `show`, what `backup show` said it holds. `preset` is `{path, time}`
 * of a backup to start from, or null; `jobs` is jobs.js's, `time` lib/time.js's
 * and `now` is in milliseconds. Resolves when the dialog closes.
 */
export function openRestore(view, {
  place, show, preset, jobs, time, now,
}) {
  const places = heldBackups(show);
  const id = uniqueId('snap-restore');
  const headingId = `${id}-h`;
  const chosen = Math.max(0, places.findIndex((one) => one.path === preset?.path));

  const placeSelect = h('select', { id: `${id}-place`, class: 'input' }, places.map((one, at) => h('option', { value: String(at), selected: at === chosen }, `${one.name} (${one.path})`)));
  const timeSelect = h('select', { id: `${id}-time`, class: 'input', 'aria-describedby': `${id}-time-hint` });
  const command = h('div', { class: 'snap-command' });
  const errors = h('div', { class: 'snap-errors' });

  /** The arguments of the action for what is chosen now. */
  const argsNow = () => {
    const one = places[Number(placeSelect.value)];
    return { name: place.name, place: one.path, at: timeSelect.value };
  };

  /** The times of the chosen place, newest first, with the one asked for (or the newest) chosen. */
  function drawTimes(wanted) {
    const one = places[Number(placeSelect.value)];
    fill(timeSelect, one.backups.map((backup, at) => h('option', { value: backup.time, selected: wanted ? backup.time === wanted : at === 0 },
      `${at === 0 ? 'Newest: ' : ''}${beside(backup.ms, time.zone, now)}`)));
  }
  let timer = null;
  const showCommand = () => {
    clearTimeout(timer);
    timer = setTimeout(() => fill(command, h('h3', {}, 'This runs'), view.api.panel('backups.restore', argsNow())), 200);
  };
  view.signal.addEventListener('abort', () => clearTimeout(timer), { once: true });
  placeSelect.addEventListener('change', () => { drawTimes(null); showCommand(); });
  timeSelect.addEventListener('change', showCommand);

  const restoreButton = h('button', { class: 'btn default', type: 'submit' }, 'Restore');
  const cancel = h('button', { class: 'btn', type: 'button' }, 'Cancel');
  const body = places.length === 0
    ? h('p', { class: 'muted' }, 'This backup place holds no backup yet, so there is nothing to restore.')
    : [
      h('p', {}, 'The backup is received into the snapshots of the place as restored-TIME. Nothing is overwritten. When it is done, this page shows it, and you pick the files you want back.'),
      h('div', { class: 'field' }, h('label', { for: `${id}-place` }, 'Place'), placeSelect),
      h('div', { class: 'field' }, h('label', { for: `${id}-time` }, 'Time of the backup'), timeSelect,
        h('span', { id: `${id}-time-hint`, class: 'hint' }, 'Newest first. A backup in a chain is received with the ones it follows, so an older one takes longer.')),
      errors, command];
  const dialog = h('dialog', { class: 'dialog', 'aria-labelledby': headingId },
    h('form', { method: 'dialog', novalidate: true },
      head(headingId, 'undo', `Restore files from ${place.name}`),
      h('div', { class: 'dialog-body' }, body),
      h('div', { class: 'dialog-foot' }, cancel, places.length > 0 && restoreButton)));

  if (places.length > 0) {
    drawTimes(preset?.time ?? null);
    showCommand();
  }
  let working = false;
  cancel.addEventListener('click', () => dialog.close('cancel'));
  dialog.addEventListener('cancel', (event) => {
    if (working) event.preventDefault();
  });
  dialog.querySelector('form').addEventListener('submit', async (event) => {
    event.preventDefault();
    if (places.length === 0) return;
    working = true;
    const done = await busy([cancel], () => jobs.start('restore', argsNow(), { button: restoreButton, where: { slot: errors } })).catch(() => ({ ok: false }));
    working = false;
    if (done.ok) dialog.close('restoring');
  });
  return openModal(dialog);
}
