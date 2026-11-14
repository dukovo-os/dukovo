// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Unlock (SignIn board 3): an administrator proves who they are again, and
// for 15 minutes the console makes changes as an administrator. The state
// is kept here: the seconds left that the server last gave, counted down in
// the browser's own clock, so that this page's clock never matters. The
// header's control and the dialog both read and set it.

import { post } from './api.js';
import { busy, clearError, errorSlot, h, icon, showError } from './dom.js';
import { canUsePasskeys, getAssertion, passkeyProblem } from './webauthn.js';

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

// What the dialog offers for each way of proving who someone is, with the
// field it asks for and what the link to it says.
const METHODS = {
  passkey: { link: 'Use your passkey', submit: 'Unlock with your passkey' },
  code: {
    link: 'Use the code from your app',
    label: 'Six-digit code',
    submit: 'Unlock',
  },
  password: { link: 'Use your password', label: 'Password', submit: 'Unlock' },
};

/** The ways this session can unlock, in the order they are offered. */
function methodsOf({ kinds }) {
  return [
    kinds.includes('passkey') && canUsePasskeys() && 'passkey',
    kinds.includes('app') && 'code',
    'password',
  ].filter(Boolean);
}

function proofField(method) {
  const isCode = method === 'code';
  const input = h('input', {
    id: 'proof', class: isCode ? 'input mono code' : 'input', required: true,
    type: isCode ? 'text' : 'password', inputmode: isCode ? 'numeric' : null,
    autocomplete: isCode ? 'one-time-code' : 'current-password',
  });
  return { input, field: h('div', { class: 'field' }, h('label', { for: 'proof' }, METHODS[method].label), input) };
}

/**
 * The body of an unlock request for `method`, with the passkey prompt for
 * it done first. `signal` ends the prompt when the dialog goes.
 */
async function proofOf(method, input, signal) {
  if (method === 'passkey') {
    const { publicKey } = await post('/api/1/unlock/passkey/options', {});
    return { kind: 'passkey', credential: await getAssertion(publicKey, signal) };
  }
  if (method === 'code') return { kind: 'code', code: input.value.replace(/\s/g, '') };
  return { kind: 'password', password: input.value };
}

/**
 * Opens the unlock dialog for `session` and resolves with whether the
 * console is unlocked when it closes. `action` is the words for what the
 * person is about to do ("restart nginx"), if there is one.
 */
export function askToUnlock(session, action) {
  if (isUnlocked()) return Promise.resolve(true);
  const methods = methodsOf(session);
  const dialog = h('dialog', { class: 'dialog medium', 'aria-labelledby': 'h-unlock' });
  const prompt = new AbortController();

  function show(method) {
    const problem = errorSlot('unlock-error');
    const status = h('p', { class: 'hint', role: 'status' });
    const { input, field } = method === 'passkey' ? {} : proofField(method);
    const submit = h('button', { class: 'btn default block', type: 'submit' },
      method === 'passkey' && icon('key'), METHODS[method].submit);
    const others = methods.filter((other) => other !== method).map((other) =>
      h('button', { class: 'btn link small', type: 'button', onclick: () => show(other) }, METHODS[other].link));
    const cancel = h('button', { class: 'btn', type: 'button', onclick: () => dialog.close() }, 'Cancel');

    const form = h('form', {},
      h('div', { class: 'dialog-head' }, icon('unlock'), h('h2', { id: 'h-unlock' }, 'Unlock to make changes')),
      h('div', { class: 'dialog-body' },
        h('p', {},
          action && ['You are about to ', h('b', {}, action), '. '],
          `Changes to ${session.host} need an administrator to prove it is them, just as `,
          h('span', { class: 'mono' }, 'elevate'), ' asks at the console.'),
        field,
        submit,
        problem,
        status,
        h('div', { class: 'btns apart' }, ...others),
        h('div', { class: 'field' },
          h('b', { class: 'label' }, 'Stay unlocked'),
          h('span', {}, 'For 15 minutes'),
          h('span', { class: 'hint' },
            'The header shows the minutes left, and one tap adds 15 more while you work. Locks sooner if you ',
            'sign out or close every console tab. Every change while unlocked is in the journal.'))),
      h('div', { class: 'dialog-foot' }, cancel));

    form.addEventListener('submit', (event) => {
      event.preventDefault();
      busy([submit, ...others], async () => {
        clearError(problem, input);
        if (method === 'passkey') status.textContent = 'Follow the browser\'s prompt to use your passkey.';
        try {
          const { unlock_seconds: seconds } = await post('/api/1/unlock', await proofOf(method, input, prompt.signal));
          setUnlockSeconds(seconds);
          dialog.close('unlocked');
        } catch (error) {
          if (!prompt.signal.aborted) showError(problem, passkeyProblem(error), input);
        } finally {
          status.textContent = '';
        }
      });
    });
    dialog.replaceChildren(form);
    (input ?? submit).focus();
  }

  return new Promise((resolve) => {
    dialog.addEventListener('close', () => {
      prompt.abort();
      dialog.remove();
      resolve(dialog.returnValue === 'unlocked');
    });
    document.body.append(dialog);
    dialog.showModal();
    show(methods[0]);
  });
}

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
      element.replaceChildren(
        icon('unlock'),
        h('span', {}, 'Unlocked · ', minutes),
        button('Reset to 15 min', () => change('/api/1/unlock/extend')),
        button('Lock', () => change('/api/1/lock')));
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
      if (unlocked) minutes.textContent = `${minutesLeft()} min`;
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
