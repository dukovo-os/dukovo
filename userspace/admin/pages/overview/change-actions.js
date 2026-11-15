// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What a person does with one change of the journal, in the Overview's What
// changed card and on the journal page of Reliability: keep a hand edit, put
// a change back, look at it. Keeping and putting back are for an
// administrator, with the console unlocked: they are routes of the journal,
// not actions of the registry, so the unlock and the retry after a lock that
// fell in between are done here as lib/action.js does them for an action.

import { busy, h } from '../../dom.js';
import { askToUnlock } from '../../unlock.js';
import { changeDialog, putBackDialog } from './change-dialogs.js';

/**
 * The buttons of a change. `done()` is called after a change was kept or put
 * back, to read what it alters again; `prefix` names the page's buttons, so
 * that each has an id that a page drawn again keeps (dom.js's focusBack).
 * Gives `buttons(change, row, detail)`, the buttons that fit one change:
 * `row` is change-model.js's `changeRow` of it and `detail` is its own
 * answer (`GET /api/1/changes/{id}`) or null when it was not read. A page
 * with many rows reads no detail for them and sets `lazy`: a change that
 * touched files is then offered to be put back, and the answer is read when
 * the person asks, which says why when it cannot be.
 */
export function makeChangeActions(view, { done, prefix, lazy = false }) {
  /** Runs `call` for an administrator whose console is unlocked, and again once if the lock fell between the dialog and the call. */
  async function unlocked(call, words) {
    if (!(await askToUnlock(view.session, words))) return false;
    try {
      await call();
    } catch (error) {
      if (error?.id !== 'ADM-106' || !(await askToUnlock(view.session, words))) throw error;
      await call();
    }
    return true;
  }

  async function keepIt(row, button) {
    await busy([button], async () => {
      try {
        if (!(await unlocked(() => view.api.post(`/api/1/changes/${row.id}/keep`), 'keep this change'))) return;
        view.notice(`Kept: ${row.title}. It stays as it is.`, { kind: 'ok' });
        await done();
      } catch (error) {
        view.problem(error, { button });
      }
    });
  }

  async function putBack(row, known, button) {
    await busy([button], async () => {
      try {
        if (!(await askToUnlock(view.session, 'put a change back'))) return;
        const detail = known ?? await view.api.get(`/api/1/changes/${row.id}`);
        if (detail.put_back?.possible !== true) {
          view.notice(`This change cannot be put back: ${detail.put_back?.why ?? 'the earlier text is not kept'}.`, { kind: 'warn' });
          return;
        }
        const files = (detail.files ?? []).map((file) => file.path);
        if (!(await putBackDialog({ title: row.title, command: detail.put_back.command, files }))) return;
        await unlocked(() => view.api.post(`/api/1/changes/${row.id}/put-back`), 'put a change back');
        view.notice(`Put back: ${row.title}.`, { kind: 'ok' });
        await done();
      } catch (error) {
        view.problem(error, { button });
      }
    });
  }

  async function seeChange(row, detail, button) {
    try {
      await changeDialog(detail ?? await view.api.get(`/api/1/changes/${row.id}`), view.time, row.title);
    } catch (error) {
      view.problem(error, { button });
    }
  }

  return {
    buttons(change, row, detail) {
      const admin = view.session.admin;
      const canPutBack = admin && (detail ? detail.put_back?.possible === true : lazy && change.files.length > 0);
      const button = (words, onclick, kind = '') => {
        const id = `${prefix}-${change.id}-${words.toLowerCase().replaceAll(' ', '-')}`;
        const element = h('button', { id, class: `btn small ${kind}`.trim(), type: 'button', 'aria-label': `${words}: ${row.title}` }, words);
        element.addEventListener('click', () => onclick(element));
        return element;
      };
      return [
        admin && row.unseen && button('Keep it', (element) => keepIt(row, element)),
        canPutBack && button(row.hand ? 'Put back' : 'Undo', (element) => putBack(row, detail, element)),
        button('See the change', (element) => seeChange(row, detail, element), 'link'),
      ];
    },
  };
}
