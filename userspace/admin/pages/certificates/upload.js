// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The dialog that uploads a certificate the person made: its name, the
// certificate and its chain, and the private key, each from a file or pasted.
// The key is a secret. It lives in its box (or in the file the person chose)
// until the button is pressed; it is read then, checked, taken out of the
// box, sent once in the body of the action as part of one document, and the
// string is let go whatever came back. It is never put in the page's text,
// a log, `localStorage` or an address, and every way out of the dialog (sent,
// refused, cancelled, closed, the page left) empties the boxes.

import { clearError, errorSlot, h, showError } from '../../dom.js';
import { openFormDialog } from '../../lib/form-dialog.js';
import { askToUnlock, isUnlocked } from '../../unlock.js';
import { kindOf } from './model.js';
import {
  addArgs, documentOf, nameProblem, pemProblem, shownArgs,
} from './upload-model.js';

let made = 0;

/**
 * A box for a PEM: a textarea that is not named, and a picker of a file.
 * A file that is chosen replaces what was typed and is read only when
 * `read()` is called. Gives `{element, area, read, clear, fault}`.
 */
function makePemBox({ label, hint, signal }) {
  made += 1;
  const id = `upload-pem-${made}`;
  const area = h('textarea', {
    id, class: 'input mono certs-pem', rows: '5', spellcheck: 'false', autocomplete: 'off', autocapitalize: 'off', autocorrect: 'off', 'aria-describedby': `${id}-hint ${id}-error`,
  });
  const picker = h('input', { id: `${id}-file`, class: 'certs-file', type: 'file', accept: '.pem,.crt,.cer,.key,.txt,text/plain' });
  const error = errorSlot(`${id}-error`);
  let file = null;

  picker.addEventListener('change', () => {
    file = picker.files?.[0] ?? null;
    area.value = '';
    area.disabled = file !== null;
    clearError(error, area);
  });
  area.addEventListener('input', () => clearError(error, area));

  /** What is in the box now: the chosen file's text, or what was typed; the box keeps it. */
  async function read() {
    return file === null ? area.value : file.text();
  }

  /** Empties the box and lets go of the file. */
  function clear() {
    area.value = '';
    area.disabled = false;
    file = null;
    try {
      picker.value = '';
    } catch {
      // A picker that refuses to be emptied is let go of with the dialog.
    }
  }
  signal.addEventListener('abort', clear, { once: true });

  return {
    area,
    read,
    clear,
    fault: (words) => showError(error, { title: words, detail: '', fix: '' }, area),
    element: h('div', { class: 'field' },
      h('label', { for: id }, label), area,
      h('label', { class: 'small', for: `${id}-file` }, 'Or choose a file'), picker,
      h('span', { id: `${id}-hint`, class: 'hint' }, hint), error),
  };
}

/**
 * Opens the dialog. `name` and `replace` are set when it was opened from a
 * row that was added by hand. Resolves, when it closes, with whether a
 * certificate was added.
 */
export async function openUpload(ctx, { name = '', replace = false } = {}) {
  const { view } = ctx;
  const nameInput = h('input', {
    id: 'upload-name', class: 'input mono', type: 'text', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off', readonly: replace, 'aria-describedby': 'upload-name-hint upload-name-error',
  });
  nameInput.value = name;
  const nameError = errorSlot('upload-name-error');
  const chain = makePemBox({
    label: 'The certificate and its chain',
    hint: 'In PEM: the server\'s certificate first, then the intermediate certificates.',
    signal: view.signal,
  });
  const key = makePemBox({
    label: 'The private key',
    hint: 'In PEM. The browser sends it once, as a secret, and keeps nothing of it.',
    signal: view.signal,
  });
  const untrusted = h('input', { type: 'checkbox', id: 'upload-untrusted' });
  const replaces = h('p', { class: 'hint', role: 'status' });

  /** The certificate of the name typed that was added by hand, which the upload replaces. */
  const existing = () => (ctx.data.list ?? []).find((one) => one.name === nameInput.value.trim()) ?? null;
  const replacing = () => (existing() !== null && kindOf(existing()) === 'added') || replace;
  const showArgs = () => shownArgs({ name: nameInput.value.trim(), replace: replacing(), untrusted: untrusted.checked });
  nameInput.addEventListener('input', () => {
    clearError(nameError, nameInput);
    const there = existing();
    replaces.textContent = there !== null && kindOf(there) === 'added' ? `${there.name} was added by hand: this replaces it.` : '';
  });

  const clearAll = () => {
    chain.clear();
    key.clear();
  };

  const closed = openFormDialog(view, {
    title: replace ? `Replace ${name}` : 'Upload your own certificate',
    icon: 'key',
    lead: ['Add a certificate you got somewhere else, with its key. This server checks that the chain leads to an authority it trusts and that the key is the certificate\'s.'],
    fields: [
      h('div', { class: 'field' }, h('label', { for: nameInput.id }, 'The certificate\'s name'), nameInput,
        h('span', { id: 'upload-name-hint', class: 'hint' }, 'A domain name, such as intranet.example.org.'), replaces, nameError),
      chain.element,
      key.element,
      h('label', { class: 'check', for: untrusted.id }, untrusted, h('span', {}, 'It is not from an authority this server trusts (a private authority)')),
    ],
    label: replace ? 'Replace' : 'Upload',
    command: () => (nameProblem(nameInput.value.trim()) === '' ? { action: 'certificate.add', args: showArgs() } : null),
    async submit(confirm, slot) {
      const wanted = nameInput.value.trim();
      const bad = nameProblem(wanted);
      if (bad !== '') {
        showError(nameError, { title: bad, detail: '', fix: '' }, nameInput);
        return false;
      }
      const there = existing();
      if (there !== null && kindOf(there) !== 'added') {
        showError(nameError, { title: `${wanted} came from an authority: remove it first, or choose another name.`, detail: '', fix: '' }, nameInput);
        return false;
      }
      // An administrator whose console is locked unlocks before the key is read, so that it waits for no dialog.
      if (!isUnlocked() && !(await askToUnlock(view.session, 'add a certificate'))) return false;
      // What was read lives in these two until they are let go, whatever came back.
      const text = { chain: '', key: '', pem: '' };
      let args = null;
      try {
        text.chain = await chain.read();
        text.key = await key.read();
        const problem = pemProblem(text.chain, text.key);
        if (problem !== null) {
          (problem.field === 'key' ? key : chain).fault(problem.words);
          return false;
        }
        text.pem = documentOf(text.chain, text.key);
        // The key leaves its box now: after a refusal the chain is still there, and the key is chosen again.
        key.clear();
        args = addArgs({ name: wanted, replace: replacing(), untrusted: untrusted.checked }, text.pem);
        const result = await view.api.run('certificate.add', args, {
          button: confirm,
          where: { slot, fields: { name: nameInput } },
          words: {
            unlock: 'add a certificate', running: `Adding ${wanted}`, busy: 'Adding…', done: `${wanted} is added.`,
          },
        });
        if (!result.ok) return false;
        await ctx.reread();
        return true;
      } finally {
        if (args !== null) args.pem = '';
        text.chain = '';
        text.key = '';
        text.pem = '';
      }
    },
  });
  return closed.finally(clearAll);
}
