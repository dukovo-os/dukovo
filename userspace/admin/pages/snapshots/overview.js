// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Snapshots and backups page gives the Overview (design 5.11): the
// things to look at, from the backup places and the snapshots, and the step
// "Copy your data off this server" of Finish setting up and /setup, and the
// step that offers Move to new hardware on a machine that is new. Every read
// is the person's own. The form of the first step is read when /setup first
// shows it, since it is the form of the page's own dialog.

import { h } from '../../dom.js';
import {
  hasGoodBackup, isNewInstall, needsOf, stepSummary,
} from './overview-model.js';

/** The action that adds a backup place. */
const ADD = 'backups.add';

const readBackups = (cx) => cx.cache('/api/1/backups', 5000, () => cx.api.get('/api/1/backups'));
const readSnapshots = (cx) => cx.cache('/api/1/snapshots', 5000, () => cx.api.get('/api/1/snapshots'));
const readAccounts = (cx) => cx.cache('/api/1/accounts', 5000, () => cx.api.get('/api/1/accounts'));

/** What the step Bring everything from your old machine says, and is called. */
const MOVE_TITLE = 'Bring everything from your old machine';
const MOVE_SUMMARY = 'Start from the backups of your old machine, not from nothing';

/** The things to look at, for the Overview. One read that fails leaves the other's needs; both failing leaves the part out. */
export async function needs(cx) {
  const [backups, list] = await Promise.allSettled([readBackups(cx), readSnapshots(cx)]);
  if (backups.status === 'rejected' && list.status === 'rejected') throw backups.reason;
  return needsOf(backups.value ?? null, list.value ?? null, { time: cx.time, now: Date.now() });
}

export const steps = [{
  id: 'move',
  title: MOVE_TITLE,
  order: 5,
  points: 0,
  action: null,
  // A step that is never done is only worth showing where it can be used, so a read that fails offers nothing.
  async available(cx) {
    try {
      const [backups, accounts] = await Promise.all([readBackups(cx), readAccounts(cx)]);
      return isNewInstall(backups, accounts);
    } catch {
      return false;
    }
  },
  done: async () => false,
  summary: async () => MOVE_SUMMARY,
  /** There is nothing to fill in here: the step is the page that moves the machine, and `args: null` says so to /setup. */
  form: () => ({
    element: h('section', { class: 'card', 'aria-labelledby': 'move-step-h' },
      h('h2', { id: 'move-step-h', tabindex: '-1' }, MOVE_TITLE),
      h('p', {}, 'If you have the backups of your old machine, this machine can take its name, accounts, settings, apps and files from them. You choose the backups, unlock them with their recovery key and check what changes before anything is done.'),
      h('p', { class: 'muted' }, 'It is done on its own page, and nothing changes until you restart.')),
    args: null,
    href: '/move?via=setup',
  }),
}, {
  id: 'backups',
  title: 'Copy your data off this server',
  order: 30,
  points: 8,
  action: ADD,
  done: async (cx) => hasGoodBackup(await readBackups(cx)),
  summary: async (cx) => stepSummary(await readBackups(cx)),
  /**
   * The fields of Add a backup place, drawn when the code of the dialog has
   * come; until then they say so, and `args` asks for them again. `after`
   * is for /setup to call with the answer of the action, which holds the
   * recovery key: the key is shown once, as it is on the page.
   */
  form(view) {
    const element = h('section', { class: 'card', 'aria-labelledby': 'snap-step-h', 'aria-busy': 'true' },
      h('h2', { id: 'snap-step-h' }, 'Copy your data off this server'),
      h('p', { class: 'muted', role: 'status' }, 'Reading the disks…'));
    let made = null;
    import('./add-place.js').then(async (code) => {
      const [storage, backups] = await Promise.all([view.api.get('/api/1/storage').catch(() => null), view.api.get('/api/1/backups').catch(() => null)]);
      if (view.signal.aborted) return;
      made = code.makeAddForm(view, { storage, backups });
      element.replaceChildren(h('h2', { id: 'snap-step-h' }, 'Copy your data off this server'), made.element);
      element.removeAttribute('aria-busy');
    }, () => {
      element.replaceChildren(h('h2', {}, 'Copy your data off this server'), h('p', { class: 'muted', role: 'status' }, 'This step\'s form could not be shown. Reload the page.'));
      element.removeAttribute('aria-busy');
    });
    return {
      element,
      args() {
        if (made === null) throw new Error('the form is not drawn yet');
        return made.args();
      },
      async after(answer) {
        if (made !== null) await made.after(answer);
      },
    };
  },
}];
