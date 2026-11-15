// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The opened backup place (design 5.7): what it holds, what it keeps, how its
// last check went, the backups there, its recovery key and the buttons that
// back up to it, restore from it and remove it. What a place holds and its
// key are an administrator's: a reader is told so, and a console that is
// locked is asked to unlock first. `backups.show` is read when a place is
// opened, and again when a task of the place ended, never on a timer, since
// it lists a disk or a server. The page reads this file when a place is
// first opened, so that the dialogs it needs are not read before.

import { fill, h } from '../../dom.js';
import { beside } from '../../lib/snapshot-words.js';
import { ask } from '../../shell/dialogs.js';
import { askToUnlock, isUnlocked } from '../../unlock.js';
import { bars } from './body.js';
import { detailOf, heldBackups, staysWords } from './detail-model.js';
import { findPlace } from './model.js';

/** The backups of a place that are listed before Show all. */
const FEW = 5;

/**
 * Makes the opened place in `container`. `jobs` is jobs.js's; `links` says
 * which pages are in this console, `{move}`; `restore(place, show, preset)`
 * opens Restore files… and `changed()` reads the page again. Gives `{set,
 * clear, ended, unlocked}`: `set(model, place)` draws the opened place, `model`
 * being what places.js was given, and leaves it as it is when what it says has
 * not changed; `clear()` forgets the place when none is opened; `ended(name)`
 * reads again what a place holds after a task of it; `unlocked()` after the
 * console was unlocked or locked.
 */
export function makeDetail(view, { container, jobs, links, restore, changed }) {
  let model = null;
  let last = null;
  // What the opened place holds: `backups.show`'s answer, or why there is none.
  let held = { for: null, state: 'idle', show: null };
  let heldBox = null;
  let latest = 0;

  const modeOf = () => (!view.session.admin ? 'reader' : isUnlocked() ? 'open' : 'locked');

  // ---- the backups a place holds ----

  /** Draws what the opened place holds into the box that the detail has for it. */
  function drawHeld() {
    if (heldBox === null) return;
    const mode = modeOf();
    if (mode === 'reader') {
      fill(heldBox, h('span', { class: 'muted' }, 'Administrators see what a backup place holds.'));
    } else if (mode === 'locked') {
      const unlock = h('button', { class: 'btn small', type: 'button' }, 'Unlock');
      unlock.addEventListener('click', async () => {
        if (await askToUnlock(view.session, 'see what a backup place holds')) loadHeld(held.for);
      });
      fill(heldBox, h('span', { class: 'muted' }, 'Unlock to see the backups there. '), unlock);
    } else if (held.state === 'loading' || held.state === 'idle') {
      fill(heldBox, bars());
    } else if (held.state === 'failed') {
      const again = h('button', { class: 'btn small', type: 'button' }, 'Try again');
      again.addEventListener('click', () => loadHeld(held.for));
      fill(heldBox, h('span', { class: 'muted' }, 'What it holds could not be read. '), again);
    } else {
      const places = heldBackups(held.show);
      if (places.length === 0) {
        fill(heldBox, h('span', { class: 'muted' }, 'Nothing is there yet.'));
        return;
      }
      fill(heldBox, ...places.map((place) => placeOf(place)));
    }
    heldBox.removeAttribute('aria-busy');
  }

  /** The backups of one place of what is held, the newest few and the rest behind Show all. */
  function placeOf(place) {
    const { time, now } = model;
    const line = (backup) => h('li', {},
      h('span', {}, beside(backup.ms, time.zone, now)),
      view.session.admin && (() => {
        const button = h('button', { class: 'btn small', type: 'button', 'aria-label': `Restore the backup of ${place.name} from ${beside(backup.ms, time.zone, now)}` }, 'Restore this…');
        button.addEventListener('click', () => restore(findPlace(model.backups, held.for), held.show, { path: place.path, time: backup.time }));
        return button;
      })());
    const rest = place.backups.slice(FEW);
    const more = h('ul', { class: 'snap-held-list', hidden: true }, rest.map(line));
    const toggle = h('button', { class: 'btn link small', type: 'button', 'aria-expanded': 'false' }, `Show all ${place.backups.length}`);
    toggle.addEventListener('click', () => {
      more.hidden = !more.hidden;
      toggle.setAttribute('aria-expanded', String(!more.hidden));
      toggle.textContent = more.hidden ? `Show all ${place.backups.length}` : 'Show fewer';
    });
    return h('div', { class: 'snap-held' },
      h('b', {}, `${place.name}, ${place.backups.length === 1 ? '1 backup' : `${place.backups.length} backups`}`),
      h('ul', { class: 'snap-held-list' }, place.backups.slice(0, FEW).map(line)),
      rest.length > 0 && [more, toggle]);
  }

  /** Reads what the place `name` holds. Only the last read counts. */
  async function loadHeld(name) {
    latest += 1;
    const mine = latest;
    held = { for: name, state: 'loading', show: null };
    if (modeOf() !== 'open') {
      held.state = 'idle';
      drawHeld();
      return;
    }
    drawHeld();
    const done = await view.api.run('backups.show', { name }, { words: { unlock: 'see what a backup place holds' }, where: { slot: heldBox } }).catch(() => ({ ok: false }));
    if (mine !== latest) return;
    held = done.ok ? { for: name, state: 'ready', show: done.result } : { for: name, state: 'failed', show: null };
    drawHeld();
  }

  // ---- the recovery key, removing a place ----

  /** Shows the recovery key again, once. */
  async function showAgain(name, pressed) {
    view.slot.replaceChildren();
    const done = await view.api.run('backups.key', { name }, { words: { unlock: 'show the recovery key', busy: 'Reading…' }, button: pressed }).catch(() => ({ ok: false }));
    if (!done.ok) return;
    const { showRecoveryKey } = await import('./key.js');
    const answer = done.result;
    done.result = null;
    await showRecoveryKey(view, { name, answer });
  }

  /** Asks, and removes the place. */
  async function remove(place, pressed) {
    const choice = await ask({
      title: `Remove ${place.name}?`,
      words: [
        `Its backups stay ${staysWords(place)}; only its recovery key reads them then.`,
        'The key kept on this machine is removed with the place. If you have not kept a copy, show it again first: without it the backups cannot be read.',
      ],
      iconName: 'warn',
      choices: [{ label: 'Cancel', value: 'cancel', safe: true }, { label: 'Remove', value: 'remove', kind: 'danger' }],
    });
    if (choice !== 'remove') return;
    const done = await view.api.run('backups.remove', { name: place.name }, {
      words: { unlock: `remove ${place.name}`, done: `Removed ${place.name}.`, busy: 'Removing…' }, button: pressed,
    }).catch(() => ({ ok: false }));
    if (!done.ok) return;
    view.go('/snapshots/backups');
    changed();
  }

  /** Opens Restore files…, reading what the place holds first when the page has not. */
  async function pressRestore(place) {
    if (held.state !== 'ready' || held.for !== place.name) {
      const done = await view.api.run('backups.show', { name: place.name }, { words: { unlock: 'restore files from a backup place' } }).catch(() => ({ ok: false }));
      if (!done.ok) return;
      held = { for: place.name, state: 'ready', show: done.result };
      drawHeld();
    }
    restore(place, held.show, null);
  }

  /** A button that runs `work` for the place and takes its own press. */
  function button(words, work, { kind = '' } = {}) {
    const one = h('button', { class: `btn ${kind}`.trim(), type: 'button' }, words);
    one.addEventListener('click', () => work(one));
    return one;
  }

  /** The opened place: what it is, what it holds and keeps, its key and its buttons. */
  function drawDetail(place) {
    const { time, now } = model;
    const words = detailOf(place, { time, now });
    const admin = view.session.admin;
    heldBox = h('div', { class: 'snap-held-box', 'aria-busy': 'true' });
    const move = links.move && admin && h('a', { class: 'btn', href: `/move?from=${encodeURIComponent(place.name)}` }, 'Restore the whole machine…');
    fill(container,
      h('div', { class: 'snap-detail-head' },
        h('h3', { id: 'snap-open-h', tabindex: '-1' }, place.name),
        h('a', { class: 'btn link small', href: '/snapshots/backups' }, 'Close')),
      h('dl', { class: 'rows' },
        h('dt', {}, 'Holds'), h('dd', {}, words.holds),
        h('dt', {}, 'Keeps'), h('dd', {}, words.keeps || 'Not said'),
        h('dt', {}, 'Restore check'), h('dd', {}, h('span', { class: words.check.tone === 'warn' ? 'warn' : '' }, words.check.words), admin && jobs.button('verify', place.name, 'Check now', { size: 'small' })),
        h('dt', {}, 'Backups there'), h('dd', {}, heldBox),
        h('dt', {}, 'Recovery key'), h('dd', {}, words.key.words, admin && [' · ', button('Show it again', (one) => showAgain(place.name, one), { kind: 'link small' })])),
      admin && h('div', { class: 'btns' },
        jobs.button('run', place.name, 'Back up now'),
        button('Restore files…', () => pressRestore(place)),
        move,
        button('Remove…', (one) => remove(place, one), { kind: 'danger' })));
    drawHeld();
  }

  return {
    set(next, place) {
      model = next;
      const key = JSON.stringify([next.open, detailOf(place, next), view.session.admin, isUnlocked()]);
      if (key === last) return;
      last = key;
      drawDetail(place);
      if (held.for !== next.open || (held.state === 'idle' && modeOf() === 'open')) loadHeld(next.open);
    },
    clear() {
      last = null;
      latest += 1;
      heldBox = null;
      held = { for: null, state: 'idle', show: null };
    },
    ended(name) {
      if (model?.open === name) loadHeld(name);
    },
    unlocked() {
      last = null;
    },
  };
}
