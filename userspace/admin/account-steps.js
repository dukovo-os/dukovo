// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The second step of the signed-in account (Accounts board, "Second step ·
// manage"): the authenticator app, passkeys, recovery codes and trusted
// browsers, through the worker's routes under /api/1/account/second-step.
// A change asks for the account's password, unless the console is unlocked:
// authd takes either as the proof. A session that the sign-in rules limited
// to adding a second step can use only this page, and signs in again once
// one is added.

import { get, post } from './api.js';
import { busy, clearError, errorSlot, h, icon, showError, uniqueId } from './dom.js';
import { qrSvg } from './qr.js';
import { isUnlocked, onUnlockChange } from './unlock.js';
import { canUsePasskeys, createCredential, isAddress, passkeyProblem } from './webauthn.js';

const PATH = '/api/1/account/second-step';

const date = (seconds) => new Date(seconds * 1000).toLocaleDateString(undefined, { dateStyle: 'medium' });

const count = (number, noun) => `${number} ${noun}${number === 1 ? '' : 's'}`;

/** The secret of an `otpauth://` address, in groups of four that are easy to key in. */
function secretIn(uri) {
  const secret = new URL(uri).searchParams.get('secret') ?? '';
  return (secret.match(/.{1,4}/g) ?? []).join(' ');
}

/**
 * The field for the account's password that a change asks for, hidden
 * while the console is unlocked and shown again if it locks.
 */
function passwordProof() {
  const id = uniqueId('proof');
  const input = h('input', { id, class: 'input', type: 'password', autocomplete: 'current-password' });
  const field = h('div', { class: 'field' },
    h('label', { for: id }, 'Your password'),
    input,
    h('span', { class: 'hint' }, 'The console is locked, so it asks for your password. Unlock it to skip this.'));
  const update = () => {
    field.hidden = isUnlocked();
    input.required = !field.hidden;
  };
  const stop = onUnlockChange(() => (field.isConnected ? update() : stop()));
  update();
  return { field, input, value: () => (isUnlocked() ? '' : input.value) };
}

/**
 * A form that opens under a card's button. `run` is called with the proof
 * (the password, or an empty string when unlocked) and a function that
 * puts a line of progress under the form; an error it throws is shown in
 * the form.
 */
function actionForm({ title, text, fields = [], withPassword, label, danger, run, cancel }) {
  const proof = withPassword ? passwordProof() : null;
  const problem = errorSlot(uniqueId('error'));
  const status = h('p', { class: 'hint', role: 'status' });
  const submit = h('button', { class: danger ? 'btn danger' : 'btn default', type: 'submit' }, label);
  const form = h('form', { class: 'panel', 'aria-label': title },
    h('h3', {}, title),
    text && h('p', { class: 'muted' }, text),
    ...fields,
    proof?.field,
    problem,
    h('div', { class: 'btns' }, submit, h('button', { class: 'btn', type: 'button', onclick: cancel }, 'Cancel')),
    status);
  form.addEventListener('submit', (event) => {
    event.preventDefault();
    busy([submit], async () => {
      clearError(problem);
      try {
        await run(proof?.value() ?? '', (words) => { status.textContent = words; });
      } catch (error) {
        showError(problem, passkeyProblem(error));
      } finally {
        status.textContent = '';
      }
    });
  });
  return form;
}

function nameField(value = '') {
  const id = uniqueId('name');
  const input = h('input', { id, class: 'input', type: 'text', required: true, maxlength: '64', autocomplete: 'off' });
  input.value = value;
  const field = h('div', { class: 'field' },
    h('label', { for: id }, 'Name'),
    input,
    h('span', { class: 'hint' }, 'The name of the key or device, as the list shows it.'));
  return { field, input };
}

/** The recovery codes, shown once, with the ways to keep them. */
function recoveryCodes({ codes, account, host, needsStep, finished }) {
  const status = h('p', { class: 'hint', role: 'status' });
  const text = [
    `Dukovo Admin recovery codes for ${account} on ${host}`,
    'Each code works once, in place of the second step.',
    '',
    ...codes,
    '',
  ].join('\n');
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      status.textContent = 'Copied.';
    } catch {
      status.textContent = 'The browser did not allow copying. Select the codes and copy them, or download them.';
    }
  };
  const download = () => {
    const link = h('a', { href: URL.createObjectURL(new Blob([text], { type: 'text/plain' })), download: `${host}-recovery-codes.txt` });
    document.body.append(link);
    link.click();
    link.remove();
    setTimeout(() => URL.revokeObjectURL(link.href), 1000);
  };
  const heading = h('h2', { id: 'h-codes', tabindex: '-1' }, 'Your recovery codes');
  const section = h('section', { class: 'card', 'aria-labelledby': 'h-codes' },
    heading,
    h('p', {},
      h('b', {}, 'They are shown only now.'),
      ' Each code works once, in place of the second step. Keep them somewhere other than this server.'),
    h('ul', { class: 'codes' }, ...codes.map((code) => h('li', {}, code))),
    h('div', { class: 'btns' },
      h('button', { class: 'btn', type: 'button', onclick: copy }, icon('copy'), 'Copy'),
      h('button', { class: 'btn', type: 'button', onclick: download }, 'Download as text'),
      h('button', { class: 'btn default', type: 'button', onclick: finished },
        needsStep ? 'I have saved them: sign in again' : 'I have saved them')),
    status);
  return { section, heading };
}

/** The QR code of `uri`, and its secret as text for an app that cannot scan. */
function appSecret(uri) {
  let picture = null;
  try {
    picture = qrSvg(uri, 'QR code for the authenticator app. The same secret is written beside it.');
  } catch {
    // An address longer than the code holds is typed in from its secret.
  }
  return h('div', { class: 'qr-row' },
    picture,
    h('div', { class: 'qr-text' },
      h('p', {}, picture ? 'Scan the code with your authenticator app, or type this secret into it:' : 'Type this secret into your authenticator app:'),
      h('span', { class: 'secret' }, secretIn(uri)),
      h('span', { class: 'hint' }, 'Spaces do not matter. The app then shows a six-digit code that changes every 30 seconds.')));
}

/**
 * The place under a card's buttons where the form they open goes, with what
 * opens a form there and what closes it, handing focus back to the button
 * that opened it.
 */
function formPlace() {
  const place = h('div');
  return {
    place,
    open(form) {
      place.replaceChildren(form);
      form.querySelector('input, button').focus();
    },
    closeTo: (button) => () => {
      place.replaceChildren();
      button.focus();
    },
  };
}

/**
 * The page. `session` is `GET /api/1/session`'s answer; `signInAgain()` ends
 * a session that the sign-in rules limited to this page, once a second step
 * is added. Returns the elements of the page, which load their own
 * contents.
 */
export function secondStepView({ session, signInAgain }) {
  const needsStep = session.needs_step;
  const heading = h('h1', { tabindex: '-1' }, 'Second step');
  const status = h('p', { class: 'small', role: 'status' });
  const failure = errorSlot('steps-error');
  let codesShown = null;
  const cards = h('div', { class: 'cards' }, h('p', { class: 'muted' }, 'Loading…'));

  async function load() {
    try {
      render(await get(PATH));
    } catch (error) {
      cards.replaceChildren();
      showError(failure, error);
    }
  }

  // Shows what a change did, makes the page match it, and shows the codes
  // the change gave, if any.
  async function finish(words, codes = []) {
    status.textContent = words;
    await load();
    if (codes.length > 0) {
      const { section, heading: codesHeading } = recoveryCodes({
        codes,
        account: session.name,
        host: session.host,
        needsStep,
        finished: () => (needsStep ? signInAgain() : codesShown.remove()),
      });
      codesShown?.remove();
      codesShown = section;
      cards.before(section);
      codesHeading.focus();
    } else if (needsStep) {
      signInAgain();
    } else {
      heading.focus();
    }
  }

  function appCard({ app }) {
    const { place, open, closeTo } = formPlace();
    const opener = h('button', { class: 'btn', type: 'button' }, app ? 'Remove the app…' : [icon('plus'), 'Add the app']);
    const close = closeTo(opener);
    opener.addEventListener('click', () => open(app ? removeApp(close) : addApp(open, close)));
    return h('section', { class: 'card' },
      h('h2', {}, 'Authenticator app ', h('span', { class: app ? 'pill ok' : 'pill' }, app ? 'Added' : 'Not added')),
      h('p', { class: 'muted' }, 'An app on your phone that shows a six-digit code to type after your password.'),
      app && h('p', { class: 'small muted' }, `Added ${date(app.added)}`),
      h('div', { class: 'btns' }, opener),
      place);
  }

  function addApp(open, cancel) {
    return actionForm({
      title: 'Add the authenticator app',
      withPassword: true,
      label: 'Continue',
      cancel,
      run: async (proof) => {
        const { uri } = await post(`${PATH}/app/begin`, { proof });
        const code = h('input', {
          id: 'app-code', class: 'input mono code', type: 'text', required: true, inputmode: 'numeric',
          autocomplete: 'one-time-code', spellcheck: 'false',
        });
        open(actionForm({
          title: 'Add the authenticator app',
          fields: [appSecret(uri), h('div', { class: 'field' }, h('label', { for: 'app-code' }, 'Code from the app'), code)],
          label: 'Add the app',
          cancel,
          run: async () => {
            const { codes } = await post(`${PATH}/app/confirm`, { code: code.value.replace(/\s/g, '') });
            await finish('The authenticator app is added.', codes);
          },
        }));
      },
    });
  }

  function removeApp(cancel) {
    return actionForm({
      title: 'Remove the authenticator app',
      text: 'The app\'s codes stop working at once.',
      withPassword: true,
      label: 'Remove the app',
      danger: true,
      cancel,
      run: async (proof) => {
        await post(`${PATH}/app/remove`, { proof });
        await finish('The authenticator app is removed.');
      },
    });
  }

  function passkeysCard({ passkeys }) {
    const { place, open, closeTo } = formPlace();
    const adder = h('button', { class: 'btn', type: 'button' }, icon('plus'), 'Add a passkey');
    const rows = passkeys.map((passkey) => {
      const rename = h('button', { class: 'btn small', type: 'button', 'aria-label': `Rename ${passkey.name}` }, 'Rename');
      const remove = h('button', { class: 'btn small', type: 'button', 'aria-label': `Remove ${passkey.name}` }, 'Remove');
      rename.addEventListener('click', () => open(renamePasskey(passkey, closeTo(rename))));
      remove.addEventListener('click', () => open(removePasskey(passkey, closeTo(remove))));
      return h('tr', {},
        h('td', {}, h('b', {}, passkey.name), h('small', { class: 'mono' }, passkey['relying-party'])),
        h('td', {}, date(passkey.added)),
        h('td', {}, passkey.used > 0 ? date(passkey.used) : 'Never'),
        h('td', { class: 'end' }, h('div', { class: 'btns' }, rename, remove)));
    });
    const table = rows.length > 0 && h('div', { class: 'tbl-box' },
      h('table', { class: 'tbl' },
        h('caption', { class: 'sr-only' }, 'Passkeys'),
        h('thead', {}, h('tr', {},
          h('th', { scope: 'col' }, 'Name'), h('th', { scope: 'col' }, 'Added'), h('th', { scope: 'col' }, 'Last used'),
          h('th', { scope: 'col' }, h('span', { class: 'sr-only' }, 'Actions')))),
        h('tbody', {}, ...rows)));
    let why = null;
    if (canUsePasskeys()) {
      adder.addEventListener('click', () => open(addPasskey(closeTo(adder))));
    } else {
      adder.disabled = true;
      why = h('p', { class: 'small muted' }, isAddress(location.hostname)
        ? 'Passkeys work only when the console is opened by the server\'s name, not by its address.'
        : 'This browser has no passkeys.');
    }
    return h('section', { class: 'card' },
      h('h2', {}, 'Passkeys ', h('span', { class: passkeys.length > 0 ? 'pill ok' : 'pill' }, count(passkeys.length, 'passkey'))),
      h('p', { class: 'muted' }, 'A security key, or this device\'s own unlock, that proves it is you without a password.'),
      table,
      h('div', { class: 'btns' }, adder),
      why,
      place);
  }

  function addPasskey(cancel) {
    const { field, input } = nameField();
    return actionForm({
      title: 'Add a passkey',
      text: 'The browser asks for your key or device next.',
      fields: [field],
      withPassword: true,
      label: 'Add the passkey',
      cancel,
      run: async (proof, say) => {
        const { publicKey } = await post(`${PATH}/passkey/begin`, { proof });
        say('Follow the browser\'s prompt to make the passkey.');
        const credential = await createCredential(publicKey);
        const { codes } = await post(`${PATH}/passkey/add`, { name: input.value.trim(), credential });
        await finish('The passkey is added.', codes);
      },
    });
  }

  function renamePasskey({ id, name }, cancel) {
    const { field, input } = nameField(name);
    return actionForm({
      title: `Rename ${name}`,
      fields: [field],
      label: 'Rename',
      cancel,
      run: async () => {
        await post(`${PATH}/passkey/rename`, { id, name: input.value.trim() });
        await finish('The passkey is renamed.');
      },
    });
  }

  function removePasskey({ id, name }, cancel) {
    return actionForm({
      title: `Remove ${name}`,
      text: 'It can no longer be used to sign in or to unlock.',
      withPassword: true,
      label: 'Remove the passkey',
      danger: true,
      cancel,
      run: async (proof) => {
        await post(`${PATH}/passkey/remove`, { proof, id });
        await finish('The passkey is removed.');
      },
    });
  }

  function recoveryCard({ recovery }) {
    const { place, open, closeTo } = formPlace();
    const opener = h('button', { class: 'btn', type: 'button' }, 'Make new codes…');
    opener.addEventListener('click', () => open(newCodes(closeTo(opener))));
    return h('section', { class: 'card' },
      h('h2', {}, 'Recovery codes ', h('span', { class: recovery ? 'pill ok' : 'pill' }, recovery ? `${recovery.left} left` : 'None')),
      h('p', { class: 'muted' }, 'Codes that each work once, in place of the second step, if the app or key is lost.'),
      h('p', { class: 'small muted' }, recovery ? `Made ${date(recovery.made)}` : 'They are made with the first second step.'),
      recovery && h('div', { class: 'btns' }, opener),
      place);
  }

  function newCodes(cancel) {
    return actionForm({
      title: 'Make new recovery codes',
      text: 'The codes there are now stop working.',
      withPassword: true,
      label: 'Make new codes',
      cancel,
      run: async (proof) => {
        const { codes } = await post(`${PATH}/codes`, { proof });
        await finish('New recovery codes are made.', codes);
      },
    });
  }

  function trustedCard({ trusted }) {
    const problem = errorSlot('trusted-error');
    const forget = h('button', { class: 'btn', type: 'button' }, 'Forget trusted browsers');
    forget.disabled = trusted === 0;
    forget.addEventListener('click', () => busy([forget], async () => {
      clearError(problem);
      try {
        const { forgotten } = await post(`${PATH}/trusted/forget`);
        await finish(`${count(forgotten, 'trusted browser')} forgotten.`);
      } catch (error) {
        showError(problem, error);
      }
    }));
    return h('section', { class: 'card' },
      h('h2', {}, 'Trusted browsers ', h('span', { class: trusted > 0 ? 'pill ok' : 'pill' }, `${trusted} trusted`)),
      h('p', { class: 'muted' }, 'Browsers that skip the second step for 30 days, because "Trust this browser" was ticked at sign-in.'),
      problem,
      h('div', { class: 'btns' }, forget));
  }

  function render(steps) {
    cards.replaceChildren(appCard(steps), passkeysCard(steps), recoveryCard(steps), trustedCard(steps));
  }

  load();
  return [
    h('div', { class: 'ph' },
      h('div', { class: 'title' },
        !needsStep && h('span', { class: 'crumb' }, h('a', { href: '/' }, 'Home'), ' › Second step'),
        heading)),
    needsStep
      ? h('div', { class: 'banner info' }, icon('info'),
        h('div', {}, h('p', {},
          h('b', {}, `The sign-in rules ask ${session.name} for a second step.`),
          ' Add one below. The console then asks you to sign in again, with the new step.')))
      : h('p', { class: 'muted' }, 'After the password, a second step proves that it is you, so that a password that leaks is not enough.'),
    status,
    failure,
    cards,
  ];
}
