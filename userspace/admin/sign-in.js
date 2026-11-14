// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The sign-in page (SignIn boards 1, 2 and 4): the account's name and
// password, a passkey alone, then the second step the account asks for. The
// session's cookie is set by the server when the last of them is done; this
// page only tells the router that it is.

import { get, post } from './api.js';
import { busy, clearError, errorSlot, h, icon, marks, showError } from './dom.js';
import { canUsePasskeys, getAssertion, isAddress, passkeyProblem } from './webauthn.js';

// What each way of finishing the second step says, and what its link says
// from another way. The order is the order they are offered in.
const WAYS = {
  app: {
    title: 'Enter the code from your app',
    label: 'Six-digit code',
    link: 'Use the code from your app',
  },
  passkey: { title: 'Use your passkey', link: 'Use a passkey instead' },
  recovery: {
    title: 'Enter a recovery code',
    label: 'Recovery code',
    hint: 'One of the codes made with the second step. Each works once.',
    link: 'Use a recovery code',
  },
};

const PASSKEYS_NEED_A_NAME =
  'Passkeys work only when this page is opened by the server\'s name, not by its address.';

/** The mark, the console's name and the machine's, at the top of a sign-in box. */
function lockup(machine) {
  return h('div', { class: 'lockup' },
    ...marks(26, 34),
    h('div', {}, h('b', {}, 'Dukovo Admin'), h('br'), machine));
}

/** A banner for what the router tells the page, such as that the session ended. */
function noticeBanner({ title, detail }) {
  return h('div', { class: 'banner warn', role: 'status' },
    icon('warn'), h('div', {}, h('p', {}, h('b', {}, title), ' ', detail)));
}

/** The note on the machine's own certificate, with the fingerprint to compare. */
function fingerprintNote(fingerprint) {
  return h('div', { class: 'banner info' },
    icon('info'),
    h('div', {},
      h('p', {},
        h('b', {}, 'First visit to this server?'),
        ' Until it has a domain, it uses its own certificate, so the browser warns once. ',
        'Check that the fingerprint the browser shows matches the one printed at the server\'s ',
        'console when it starts, or by ', h('span', { class: 'mono' }, 'admin fingerprint'), ':'),
      h('span', { class: 'mono fingerprint' }, fingerprint)));
}

/**
 * Draws the sign-in page into `page` and focuses its first field. `notice`,
 * if given, is `{title, detail}` words for why the person is here again;
 * `onSignedIn` is called once the server has set the session's cookie.
 */
export function showSignIn(page, { notice, onSignedIn }) {
  const machine = h('span', { class: 'muted' }, location.hostname);
  const dialogs = h('div', { class: 'sign-in-dialogs' });
  const note = h('div', { class: 'sign-in-note' });
  page.replaceChildren(h('main', { class: 'sign-in' }, dialogs, note));
  document.title = 'Sign in · Dukovo Admin';

  // The note is an extra: a page that cannot load it still signs people in.
  get('/api/1/fingerprint')
    .then(({ host, fingerprint }) => {
      machine.textContent = host;
      note.replaceChildren(fingerprintNote(fingerprint));
    })
    .catch(() => {});

  function show(form, first) {
    dialogs.replaceChildren(form);
    first.focus();
  }

  function credentialsForm() {
    const account = h('input', {
      id: 'account', class: 'input', type: 'text', required: true,
      autocomplete: 'username', autocapitalize: 'none', spellcheck: 'false',
    });
    const password = h('input', {
      id: 'password', class: 'input', type: 'password', required: true, autocomplete: 'current-password',
    });
    const problem = errorSlot('sign-in-error');
    const status = h('p', { class: 'hint', role: 'status' });
    const signIn = h('button', { class: 'btn default block', type: 'submit' }, 'Sign in');
    const buttons = [signIn];
    const rows = [signIn];

    if ('PublicKeyCredential' in window) {
      const withPasskey = h('button', { class: 'btn block', type: 'button' }, icon('key'), 'Sign in with a passkey');
      if (isAddress(location.hostname)) {
        withPasskey.disabled = true;
        rows.push(withPasskey, h('p', { class: 'small muted' }, PASSKEYS_NEED_A_NAME));
      } else {
        buttons.push(withPasskey);
        rows.push(withPasskey);
        withPasskey.addEventListener('click', () => busy(buttons, async () => {
          clearError(problem, password);
          status.textContent = 'Follow the browser\'s prompt to use your passkey.';
          try {
            const { publicKey } = await post('/api/1/sign-in/passkey/options', {});
            const credential = await getAssertion(publicKey);
            await post('/api/1/sign-in/passkey', { credential });
            onSignedIn();
          } catch (error) {
            showError(problem, passkeyProblem(error));
          } finally {
            status.textContent = '';
          }
        }));
      }
    }

    const form = h('form', { class: 'dialog narrow', 'aria-labelledby': 'h-sign-in' },
      h('div', { class: 'dialog-body roomy' },
        lockup(machine),
        notice && noticeBanner(notice),
        h('h1', { id: 'h-sign-in', class: 'dialog-title' }, 'Sign in'),
        h('div', { class: 'field' }, h('label', { for: 'account' }, 'Account'), account),
        h('div', { class: 'field' },
          h('label', { for: 'password' }, 'Password'), password,
          h('span', { class: 'hint' }, 'The same password as at the server\'s console and over SSH.')),
        problem,
        ...rows,
        status,
        h('p', { class: 'small muted' },
          'Only accounts that are administrators, or that an administrator let in, can sign in here.')));

    form.addEventListener('submit', (event) => {
      event.preventDefault();
      busy(buttons, async () => {
        clearError(problem, password);
        const name = account.value.trim();
        try {
          const answer = await post('/api/1/sign-in', { name, password: password.value });
          if (answer.step) stepForm(name, answer.step);
          else onSignedIn();
        } catch (error) {
          showError(problem, error, password);
        }
      });
    });
    show(form, account);
  }

  function stepForm(name, { ticket, kinds }) {
    const ways = [
      kinds.includes('app') && 'app',
      kinds.includes('passkey') && canUsePasskeys() && 'passkey',
      kinds.includes('recovery') && 'recovery',
    ].filter(Boolean);
    let trust = true;

    function showWay(way) {
      const { title, label, hint } = WAYS[way];
      const problem = errorSlot('step-error');
      const status = h('p', { class: 'hint', role: 'status' });
      const submit = h('button', { class: 'btn default block', type: 'submit' },
        way === 'passkey' ? [icon('key'), 'Use your passkey'] : 'Continue');
      const trusting = h('input', { type: 'checkbox' });
      trusting.checked = trust;
      trusting.addEventListener('change', () => { trust = trusting.checked; });
      const others = ways.filter((other) => other !== way).map((other) =>
        h('button', { class: 'btn link small', type: 'button', onclick: () => showWay(other) }, WAYS[other].link));

      let code = null;
      if (way !== 'passkey') {
        code = h('input', {
          id: 'code', required: true, spellcheck: 'false', autocapitalize: 'none',
          class: way === 'app' ? 'input mono code' : 'input mono',
          type: 'text', inputmode: way === 'app' ? 'numeric' : 'text',
          autocomplete: way === 'app' ? 'one-time-code' : 'off',
        });
      }

      const form = h('form', { class: 'dialog narrow', 'aria-labelledby': 'h-step' },
        h('div', { class: 'dialog-body roomy' },
          lockup(machine),
          h('h1', { id: 'h-step', class: 'dialog-title' }, title),
          h('p', { class: 'muted' }, `${name} turned on a second step for this account.`),
          code && h('div', { class: 'field' },
            h('label', { for: 'code' }, label), code, hint && h('span', { class: 'hint' }, hint)),
          h('label', { class: 'check' }, trusting, h('span', {}, 'Trust this browser for 30 days')),
          problem,
          submit,
          status,
          h('div', { class: 'btns apart' }, ...others)));

      form.addEventListener('submit', (event) => {
        event.preventDefault();
        busy([submit, ...others], async () => {
          clearError(problem, code);
          try {
            if (way === 'passkey') {
              status.textContent = 'Follow the browser\'s prompt to use your passkey.';
              const { publicKey } = await post('/api/1/sign-in/passkey/options', { ticket });
              const credential = await getAssertion(publicKey);
              await post('/api/1/sign-in/passkey', { ticket, trust, credential });
            } else {
              await post('/api/1/sign-in/code', { ticket, code: code.value.replace(/[\s-]/g, ''), trust });
            }
            onSignedIn();
          } catch (error) {
            showError(problem, passkeyProblem(error), code);
          } finally {
            status.textContent = '';
          }
        });
      });
      show(form, code ?? submit);
    }

    if (ways.length > 0) {
      showWay(ways[0]);
      return;
    }
    const back = h('button', { class: 'btn default block', type: 'button', onclick: credentialsForm }, 'Back');
    show(h('form', { class: 'dialog narrow', 'aria-labelledby': 'h-step' },
      h('div', { class: 'dialog-body roomy' },
        lockup(machine),
        h('h1', { id: 'h-step', class: 'dialog-title' }, 'This browser cannot finish signing in'),
        h('p', {}, `The second step of ${name} is a passkey. ${PASSKEYS_NEED_A_NAME}`),
        back)), back);
  }

  credentialsForm();
}
