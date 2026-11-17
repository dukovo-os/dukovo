// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The unlock dialog (SignIn board 3), apart from unlock.js so that the
// console's start does not carry it, or the passkey code it needs: most
// sessions never open it. unlock.js loads it when it is first asked.

import { post } from './api.js';
import { busy, clearError, errorSlot, h, icon, sheet, showError } from './dom.js';
import { isUnlocked, setUnlockSeconds } from './unlock.js';
import { canUsePasskeys, getAssertion, passkeyProblem } from './webauthn.js';

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
 * person is about to do ("restart nginx"), if there is one. The dialog
 * opens once its sheet has loaded.
 */
export async function openUnlockDialog(session, action) {
  if (isUnlocked()) return true;
  await sheet('/dialogs.css');
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
            'The header shows the minutes left, and one tap sets it back to 15 while you work. Locks sooner if you ',
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

