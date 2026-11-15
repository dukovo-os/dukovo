// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Step 1 of Move to new hardware (design 6.3): find the backup. The backups
// on plugged-in disks that `backup find` found and the backup places of this
// machine are a list to choose from, with a folder typed for a backup that
// neither lists. The disks are an administrator's to look at, so a console
// that is locked is asked to unlock first, and the places are listed
// meanwhile. The choice is the page's (`state.choice`); this file draws it,
// and says when the person went on.

import {
  errorSlot, fill, h, showError, uniqueId,
} from '../../dom.js';
import { folderFault, sourcesOf } from './model.js';

/** The id of the choice of a folder that is typed. */
export const FOLDER = 'folder';

/**
 * Step 1 in a card. `state` is the page's: `{found, backups, choice,
 * locked, reading}`. `hooks.next(choice)` is called with the backup the
 * person went on with, `hooks.look()` for Look again and `hooks.unlock()`
 * for Unlock. Gives `{element, focus, draw}`; `draw()` shows the state as
 * it is now and leaves what the person is typing as it is.
 */
export function makeFind(view, state, hooks) {
  const id = uniqueId('mv-find');
  const heading = h('h2', { id: `${id}-h`, tabindex: '-1' }, 'Find the backup');
  const words = h('p', { class: 'muted', role: 'status' });
  const unlock = h('button', { class: 'btn small', type: 'button', hidden: true }, 'Unlock');
  const list = h('fieldset', { class: 'choices' }, h('legend', { class: 'sr-only' }, 'The backups to move from'));
  const folder = h('input', {
    id: `${id}-folder`, class: 'input mono', type: 'text', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off', 'aria-describedby': `${id}-folder-hint`,
  });
  const folderError = errorSlot(`${id}-folder-error`);
  const folderField = h('div', { class: 'field', hidden: true },
    h('label', { for: folder.id }, 'Folder'),
    folder,
    h('span', { id: `${id}-folder-hint`, class: 'hint' }, 'The folder that holds a machine\'s backups, such as /volumes/BACKUP/dukovo-backup/duke-nas.'),
    folderError);
  const look = h('button', { class: 'btn', type: 'button' }, 'Look again');
  const next = h('button', { class: 'btn default', type: 'button', 'aria-disabled': 'true' }, 'Continue');
  let items = [];
  let selected = null;

  /** Whether the person may go on: a backup is chosen, and a folder that is typed is a path. */
  const ready = () => selected !== null;

  function select(value) {
    selected = value;
    for (const card of list.querySelectorAll('.radio-card')) card.classList[card.querySelector('input').value === value ? 'add' : 'remove']('on');
    folderField.hidden = value !== FOLDER;
    next.setAttribute('aria-disabled', String(!ready()));
  }

  /** One radio card: `title` over `detail`. */
  function card(value, title, detail) {
    const input = h('input', { type: 'radio', name: `${id}-from`, value, checked: value === selected });
    input.addEventListener('change', () => select(value));
    return h('label', { class: `radio-card${value === selected ? ' on' : ''}` }, input,
      h('span', { class: 'mv-choice' }, h('b', {}, title), detail && h('span', { class: 'small muted' }, detail)));
  }

  /** What the status line says, and whether Unlock is offered. */
  function statusWords(disks) {
    if (state.locked) return { text: 'Unlock the console to look for backups on the plugged-in disks.', unlock: true };
    if (state.reading) return { text: 'Looking for backups on the plugged-in disks…', unlock: false };
    if (state.found !== null && disks.length === 0) return { text: 'No backups found on the plugged-in disks. Plug in the disk with the backups, then Look again.', unlock: false };
    return { text: 'Plug in the disk with the backups, then Look again.', unlock: false };
  }

  function draw() {
    const { disks, places } = sourcesOf(state.found, state.backups, view.time);
    items = [...disks, ...places];
    // What the address or an earlier press chose stays chosen; a disk that was unplugged is not there to be.
    selected = state.choice?.kind === 'folder' ? FOLDER : (items.find((item) => item.id === state.choice?.id)?.id ?? null);
    if (state.choice?.kind === 'folder' && folder.value === '') folder.value = state.choice.source;
    const status = statusWords(disks);
    words.textContent = status.text;
    unlock.hidden = !status.unlock;
    look.setAttribute('aria-disabled', String(state.locked || state.reading));
    fill(list, h('legend', { class: 'sr-only' }, 'The backups to move from'),
      ...disks.map((item) => card(item.id, item.title, item.detail)),
      ...places.map((item) => card(item.id, item.title, item.detail)),
      card(FOLDER, 'A folder…', 'A folder of a machine\'s backups, typed'));
    folderField.hidden = selected !== FOLDER;
    next.setAttribute('aria-disabled', String(!ready()));
  }

  /** The backup chosen, or null with a reason shown: nothing is chosen, or the folder is not a path. */
  function chosen() {
    if (selected === null) return null;
    if (selected !== FOLDER) return items.find((item) => item.id === selected) ?? null;
    const fault = folderFault(folder.value);
    if (fault !== '') {
      showError(folderError, { title: fault }, folder);
      return null;
    }
    return {
      id: FOLDER, kind: 'folder', source: folder.value, machine: null, title: 'A folder', detail: folder.value,
    };
  }

  next.addEventListener('click', () => {
    if (!ready()) {
      words.textContent = 'Choose the backup to move from.';
      list.querySelector('input')?.focus();
      return;
    }
    const choice = chosen();
    if (choice !== null) hooks.next(choice);
  });
  look.addEventListener('click', () => {
    if (look.getAttribute('aria-disabled') !== 'true') hooks.look();
  });
  unlock.addEventListener('click', () => hooks.unlock());
  folder.addEventListener('input', () => {
    folderError.replaceChildren();
    folder.removeAttribute('aria-invalid');
    folder.classList.remove('bad');
  });

  const element = h('section', { class: 'card', 'aria-labelledby': heading.id },
    heading, words, h('div', { class: 'btns' }, unlock), list, folderField,
    h('div', { class: 'btns' }, look, next));
  draw();
  return { element, focus: () => heading.focus(), draw };
}
