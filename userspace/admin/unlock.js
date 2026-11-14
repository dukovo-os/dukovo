// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Unlock (SignIn board 3): an administrator proves who they are again, and
// for 15 minutes the console makes changes as an administrator. The state
// is kept here: the seconds left that the server last gave, counted down in
// the browser's own clock, so that this page's clock never matters. The
// header's control and the dialog both read and set it.

import { post } from './api.js';
import { clearError, errorSlot, h, icon, showError } from './dom.js';

// With this many minutes left the header says the unlock is about to end.
const WARN_MINUTES = 2;

const listeners = new Set();
let endsAt = null;
let shownMinutes = null;
let ticker = null;

/** Whether changes may be made now. */
export function isUnlocked() {
  return endsAt !== null && performance.now() < endsAt;
}

/** The minutes still left, counting a started one: 1 until the last second. */
function minutesLeft() {
  return Math.ceil((endsAt - performance.now()) / 60_000);
}

/** The minutes of the unlock still left, counting a started one, or null while the console is locked. */
export function unlockMinutes() {
  return isUnlocked() ? minutesLeft() : null;
}

function notify() {
  for (const listener of listeners) listener();
}

function tick() {
  if (!isUnlocked()) {
    setUnlockSeconds(null);
  } else if (minutesLeft() !== shownMinutes) {
    shownMinutes = minutesLeft();
    notify();
  }
}

/**
 * Takes what the server says is left of the unlock, in seconds (null or 0
 * when the console is locked), and tells whoever follows it.
 */
export function setUnlockSeconds(seconds) {
  endsAt = seconds > 0 ? performance.now() + seconds * 1000 : null;
  shownMinutes = endsAt === null ? null : minutesLeft();
  clearInterval(ticker);
  ticker = endsAt === null ? null : setInterval(tick, 1000);
  notify();
}

/** Calls `listener` whenever the unlock changes or a minute of it passes; the returned function stops that. */
export function onUnlockChange(listener) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/**
 * Opens the unlock dialog for `session` and resolves with whether the
 * console is unlocked when it closes. `action` is the words for what the
 * person is about to do ("restart nginx"), if there is one. The dialog is
 * another module, read when it is first needed: most sessions never open it.
 */
export async function askToUnlock(session, action) {
  if (isUnlocked()) return true;
  const { openUnlockDialog } = await import('./unlock-dialog.js');
  return openUnlockDialog(session, action);
}

/** Whether to go on with Lock: it asks first when administrator shells would end. */
export const confirmLock = async () => (await import('./shell/dialogs.js')).lockOk();

/**
 * The header's control: what the console may do now, with the minutes left
 * while it is unlocked, and the buttons to lock it, to keep it unlocked for
 * 15 minutes from now, or to unlock it. It follows the unlock's state for as long as it is on the
 * page.
 */
export function accessControl(session) {
  const element = h('span', { class: 'access' });
  const problem = errorSlot('access-error');
  const announcement = h('span', { class: 'sr-only', role: 'status' });
  const minutes = h('span');
  let unlocked = null;

  // The buttons are not disabled while a call is out: that would take focus
  // from the one a person at the keyboard pressed.
  let pending = false;
  async function change(path) {
    if (pending) return;
    pending = true;
    clearError(problem);
    try {
      const { unlock_seconds: seconds } = await post(path);
      setUnlockSeconds(seconds);
    } catch (error) {
      showError(problem, error);
    } finally {
      pending = false;
    }
  }

  async function unlock() {
    await askToUnlock(session);
    element.querySelector('button')?.focus();
  }

  function rebuild() {
    unlocked = isUnlocked();
    const button = (words, onclick, label) =>
      h('button', { class: 'btn link small', type: 'button', 'aria-label': label, onclick }, words);
    element.className = unlocked ? 'access open' : 'access';
    if (unlocked) {
      minutes.textContent = `${minutesLeft()} min`;
      element.classList.toggle('soon', minutesLeft() <= WARN_MINUTES);
      element.replaceChildren(
        icon('unlock'),
        h('span', {}, 'Unlocked · ', minutes),
        button('Reset to 15 min', () => change('/api/1/unlock/extend')),
        button('Lock', async () => (await confirmLock()) && change('/api/1/lock')));
    } else {
      element.replaceChildren(
        icon('lock'),
        h('span', {}, session.admin ? 'Limited access ·' : 'Limited access'),
        session.admin && button('Unlock', unlock));
    }
  }

  function follow() {
    if (!element.isConnected) {
      stop();
    } else if (isUnlocked() === unlocked) {
      if (unlocked) {
        minutes.textContent = `${minutesLeft()} min`;
        element.classList.toggle('soon', minutesLeft() <= WARN_MINUTES);
      }
    } else {
      const hadFocus = element.contains(document.activeElement);
      rebuild();
      announcement.textContent = unlocked ? 'Unlocked.' : 'Locked.';
      if (hadFocus) element.querySelector('button')?.focus();
    }
  }

  rebuild();
  const stop = onUnlockChange(follow);
  return h('span', { class: 'access-box' }, element, problem, announcement);
}
