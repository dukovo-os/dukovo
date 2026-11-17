// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The shell's own dialogs: a question with choices, the keyboard shortcuts
// and About. Each is a modal <dialog> named by its heading; when one closes,
// focus goes back to what opened it (dom.js's focusBack).

import { get as fetchJson } from '../api.js';
import { focusBack, h, icon, sheet, uniqueId } from '../dom.js';

/** Opens `dialog` as a modal, once its sheet has loaded, and resolves with its return value when it closes. */
export async function openModal(dialog) {
  const opener = document.activeElement;
  await sheet('/dialogs.css');
  return new Promise((resolve) => {
    dialog.addEventListener('close', () => {
      dialog.remove();
      focusBack(opener);
      resolve(dialog.returnValue);
    });
    document.body.append(dialog);
    dialog.showModal();
  });
}

/** The head of a dialog: its icon and its heading, which names the dialog. */
export const head = (id, iconName, title) => h('div', { class: 'dialog-head' }, icon(iconName), h('h2', { id }, title));

/**
 * Asks a question. `choices` are `{label, value, kind, safe}` with `kind`
 * `'default'` or `'danger'` or nothing. The dialog opens with focus on the
 * choice marked `safe`, else on the first: a question before something
 * that ends or stops work opens on the choice that keeps it, so a key
 * pressed by chance changes nothing. Resolves with the value chosen, or
 * null when the dialog is closed with Esc.
 */
export async function ask({ title, words, choices, iconName = 'info' }) {
  const id = uniqueId('ask');
  const buttons = choices.map((choice) => h('button', {
    class: `btn ${choice.kind ?? ''}`.trim(), type: 'submit', value: choice.value, autofocus: choice.safe === true,
  }, choice.label));
  const dialog = h('dialog', { class: 'dialog medium', 'aria-labelledby': id },
    h('form', { method: 'dialog' },
      head(id, iconName, title),
      h('div', { class: 'dialog-body' }, ...[].concat(words).map((line) => h('p', {}, line))),
      h('div', { class: 'dialog-foot' }, ...buttons)));
  const value = await openModal(dialog);
  return value === '' ? null : value;
}

/** What the keys do, for the dialog and for tests: `[keys, words]`. */
export const SHORTCUTS = [
  ['Ctrl K', 'Search the console and run a command (Meta K too)'],
  ['Ctrl /', 'Show these keys'],
  ['Esc', 'Close the dialog, drawer, menu or note in front'],
  ['Arrow keys, Home, End', 'Move in a menu, a list of tabs or the search results'],
  ['Alt Shift K', 'Search, from inside a terminal'],
  ['Alt Shift Up', 'Leave a terminal pane for the terminal\'s tabs'],
];

export function shortcutsDialog() {
  const id = uniqueId('keys');
  const dialog = h('dialog', { class: 'dialog medium', 'aria-labelledby': id },
    h('form', { method: 'dialog' },
      head(id, 'info', 'Keyboard shortcuts'),
      h('div', { class: 'dialog-body' },
        h('dl', { class: 'rows' }, ...SHORTCUTS.flatMap(([keys, words]) => [
          h('dt', {}, h('span', { class: 'kbd' }, keys)), h('dd', {}, words)])),
        h('p', { class: 'small muted' }, 'There are no keys of one character, so nothing here can be pressed by mistake while typing.')),
      h('div', { class: 'dialog-foot' }, h('button', { class: 'btn default', type: 'submit' }, 'Close'))));
  return openModal(dialog);
}

/**
 * About: the release the server tells (`get` is api.js's; an older server
 * does not say) and the console build, read from the script's ETag.
 */
export async function aboutDialog({ get }) {
  const release = await get('/api/1/console/about').then((answer) => answer.release ?? null, () => null);
  const tag = await fetch('/app.js', { method: 'HEAD', credentials: 'same-origin' }).then((answer) => answer.headers.get('ETag'), () => null);
  const build = tag ? tag.replace(/^W\/|"/g, '').slice(0, 12) : null;
  const id = uniqueId('about');
  const dialog = h('dialog', { class: 'dialog medium', 'aria-labelledby': id },
    h('form', { method: 'dialog' },
      head(id, 'info', 'About Dukovo Admin'),
      h('div', { class: 'dialog-body' },
        h('dl', { class: 'rows' },
          h('dt', {}, 'Release'), h('dd', {}, release ?? 'Not told'),
          build && [h('dt', {}, 'Console build'), h('dd', { class: 'mono' }, build)],
          h('dt', {}, 'Licence'), h('dd', {}, 'The console\'s files are Apache-2.0 or MIT, as you choose.'))),
      h('div', { class: 'dialog-foot' }, h('button', { class: 'btn default', type: 'submit' }, 'Close'))));
  return openModal(dialog);
}

/**
 * The question before signing out: terminals that run can keep running or
 * end. Gives true when the person goes on to sign out.
 */
export async function signOutOk({ get, post }) {
  let running = [];
  try {
    const answer = await get('/api/1/terminals');
    running = Array.isArray(answer) ? answer : answer?.terminals ?? [];
  } catch {
    // A server with no terminals route has none running.
  }
  if (running.length === 0) return true;
  const count = running.length;
  const choice = await ask({
    title: 'Sign out?',
    words: `You have ${count} ${count === 1 ? 'terminal' : 'terminals'} running.`,
    iconName: 'warn',
    choices: [
      { label: 'Keep them running', value: 'keep', kind: 'default' },
      { label: 'End them and sign out', value: 'end' },
      { label: 'Cancel', value: 'cancel', safe: true },
    ],
  });
  if (choice === 'end') await Promise.allSettled(running.map((one) => post(`/api/1/terminals/${one.id}/end`)));
  return choice === 'keep' || choice === 'end';
}

/** How many administrator shells `answer`, what GET /api/1/terminals gave, lists: this console session's own, which Lock ends. */
export function adminShells(answer) {
  const terminals = Array.isArray(answer) ? answer : answer?.terminals ?? [];
  return terminals.reduce((sum, terminal) => sum + (terminal.panes ?? []).filter((pane) => pane.admin).length, 0);
}

/** The question before Lock when `count` administrator shells would end (design 13.4). */
export const lockTitle = (count) => `Lock and end ${count} administrator ${count === 1 ? 'shell' : 'shells'}?`;

/**
 * The question before Lock: administrator shells end with the unlock. Gives
 * true when the console has none or the person goes on to lock it.
 */
export async function lockOk(get = fetchJson) {
  let count = 0;
  try {
    count = adminShells(await get('/api/1/terminals'));
  } catch {
    // A server with no terminals route has no shells to end.
  }
  if (count === 0) return true;
  const choice = await ask({
    title: lockTitle(count),
    words: 'An administrator shell ends when the console locks, and what runs in it stops.',
    iconName: 'warn',
    choices: [{ label: 'Lock and end', value: 'lock', kind: 'default' }, { label: 'Cancel', value: 'cancel', safe: true }],
  });
  return choice === 'lock';
}
