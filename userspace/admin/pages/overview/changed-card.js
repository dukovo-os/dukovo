// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What changed (design 10.2): the last five changes, as the board's timeline,
// each with the buttons that fit it. A change someone made by hand and nobody
// has looked at can be kept or put back; a change through the console can be
// undone; any can be looked at. Keeping, putting back and undoing are for an
// administrator, with the console unlocked: they are routes of the journal,
// not actions of the registry, so the unlock and the retry after a lock that
// fell in between are done here as lib/action.js does them for an action.

import { busy, fill, h } from '../../dom.js';
import { askToUnlock } from '../../unlock.js';
import { linkable, makeCard, reader } from './card.js';
import { changeDialog, putBackDialog } from './dialogs.js';
import { changedHeading, changeRow } from './model.js';

/** How many changes the card lists. */
const SHOWN = 5;

/** The card. `about()` gives `console.about`'s answer or null, and `reload()` reads the cards again that a change in the journal alters. */
export function makeChanged(view, { about, reload }) {
  const card = makeCard('changed', 'What changed');
  card.heading.tabIndex = -1;
  card.more.classList.add('muted');

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
        await reload();
        card.heading.focus();
      } catch (error) {
        view.problem(error, { button });
      }
    });
  }

  async function putBack(row, detail, button) {
    await busy([button], async () => {
      try {
        if (!(await askToUnlock(view.session, 'put a change back'))) return;
        const files = (detail.files ?? []).map((file) => file.path);
        if (!(await putBackDialog({ title: row.title, command: detail.put_back.command, files }))) return;
        await unlocked(() => view.api.post(`/api/1/changes/${row.id}/put-back`), 'put a change back');
        view.notice(`Put back: ${row.title}.`, { kind: 'ok' });
        await reload();
        card.heading.focus();
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

  function itemOf(change, detail) {
    const row = changeRow(change, view.time);
    const admin = view.session.admin;
    const canPutBack = admin && detail?.put_back?.possible === true;
    // Each button has an id that the card drawn again keeps, so that focus
    // finds its way back to it after a dialog (dom.js's focusBack).
    const button = (words, onclick, kind = '') => {
      const id = `ov-change-${change.id}-${words.toLowerCase().replaceAll(' ', '-')}`;
      const element = h('button', { id, class: `btn small ${kind}`.trim(), type: 'button', 'aria-label': `${words}: ${row.title}` }, words);
      element.addEventListener('click', () => onclick(element));
      return element;
    };
    return h('div', { class: 'tl-item' },
      h('time', { datetime: new Date(change.at * 1000).toISOString(), title: view.time.full(change.at) }, row.time),
      h('span', { class: 'tl-rail' }, h('i', { class: row.dot }, h('span', { class: 'sr-only' }, { warn: 'Needs a look', bad: 'Did not work', ok: 'Kept', info: '' }[row.dot]))),
      h('div', { class: 'tl-body' },
        h('b', {}, row.title),
        h('span', { class: 'muted small' }, row.sub),
        h('span', { class: 'btns' },
          admin && row.unseen && button('Keep it', (element) => keepIt(row, element)),
          canPutBack && button(row.hand ? 'Put back' : 'Undo', (element) => putBack(row, detail, element)),
          button('See the change', (element) => seeChange(row, detail, element), 'link'))));
  }

  const load = reader(view, card.body, {
    async read() {
      const now = Math.floor(Date.now() / 1000);
      const heading = changedHeading(await about().catch(() => null), view.time, now);
      const { changes = [] } = await view.api.get(`/api/1/changes?since=${heading.since}&limit=${SHOWN}`);
      // Whether a change can be put back is in the change itself, not in the list.
      const details = view.session.admin
        ? await Promise.all(changes.map((change) => view.api.get(`/api/1/changes/${change.id}`).catch(() => null)))
        : changes.map(() => null);
      return { heading, changes, details };
    },
    draw({ heading, changes, details }) {
      card.more.textContent = heading.more;
      fill(card.body,
        changes.length === 0
          ? h('p', { class: 'muted' }, `Nothing has changed ${heading.more}.`)
          : h('div', { class: 'tl' }, ...changes.map((change, at) => itemOf(change, details[at]))),
        linkable('/reliability') && h('a', { href: '/reliability' }, 'Open the full journal'));
    },
  });
  return { element: card.element, load };
}
