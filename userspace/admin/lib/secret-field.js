// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A field for a password or a key (design 3.5 and 3.6, ADR 0561). What is
// typed lives in the input and nowhere else until `take()` hands it over
// once and empties the field: it is never kept in the page's own state, in
// `localStorage` or `sessionStorage`, in an address, in an attribute, in the
// console's log or in a message. The input has no `name`, so a form that
// submitted by itself would send nothing, and the caller sends the value
// only in the body of the action's own call, never to `POST /api/1/show`.
// Nothing runs at import.

import { clearError, errorSlot, h, showError, uniqueId } from '../dom.js';
import { lowerFirst } from './action-model.js';

// What each kind asks of the browser: a new password may be offered one to
// keep, a password that already exists (the person's own, asked for again
// before something that matters) may be filled in by the browser that holds
// it, and a key is neither remembered nor corrected.
const KINDS = Object.freeze({
  password: { autocomplete: 'new-password', class: 'input' },
  current: { autocomplete: 'current-password', class: 'input' },
  key: { autocomplete: 'off', class: 'input mono' },
});

/**
 * A field that asks for `label`. `kind` is `password` (a new one), `current`
 * (one that already exists) or `key`; `confirm` (a new password only) adds a
 * second field that must say the same; `optional`
 * lets the field stay empty, which `take()` then gives as `''`; `hint` is
 * the line under the fields; `signal` empties them when it aborts, as when
 * the page is left. Gives `{element, focus, take, fail, clear}`.
 */
export function makeSecretField({
  label, kind = 'password', confirm = false, optional = false, hint = null, signal = null,
}) {
  if (!Object.hasOwn(KINDS, kind)) throw new TypeError(`a secret field is a password, a current password or a key, not ${kind}`);
  if (confirm && kind !== 'password') throw new TypeError('only a password is typed twice');
  const how = KINDS[kind];
  const id = uniqueId('secret');
  const problem = errorSlot(`${id}-error`);

  /** One input that is not named, spelled out or corrected, with its visible label. */
  const typed = (inputId, words) => {
    const field = h('input', {
      id: inputId, class: how.class, type: 'password', autocomplete: how.autocomplete,
      spellcheck: 'false', autocapitalize: 'off', autocorrect: 'off',
    });
    return { input: field, label: h('label', { for: inputId }, words) };
  };
  const first = typed(id, label);
  const second = confirm ? typed(`${id}-again`, `${label}, again`) : null;
  const input = first.input;
  const again = second?.input ?? null;
  const inputs = [input, again].filter(Boolean);
  // Typing again takes the last refusal away.
  for (const one of inputs) one.addEventListener('input', () => inputs.forEach((other) => clearError(problem, other)));

  let shown = false;
  const toggle = h('button', {
    class: 'btn small', type: 'button', 'aria-pressed': 'false', 'aria-label': `Show ${lowerFirst(label)}`,
  }, 'Show');
  /** Shows the typing as text, or as dots again. */
  const reveal = (on) => {
    shown = on;
    for (const one of inputs) one.setAttribute('type', on ? 'text' : 'password');
    toggle.setAttribute('aria-pressed', String(on));
  };
  toggle.addEventListener('click', () => reveal(!shown));

  /** Empties every field, and hides what is typed next. */
  function clear() {
    for (const one of inputs) {
      one.value = '';
      clearError(problem, one);
    }
    reveal(false);
  }
  signal?.addEventListener('abort', clear, { once: true });

  /** Says why nothing is given, under the field, with the focus on it. */
  const refuse = (title, detail, on) => showError(problem, {
    id: null, title, detail, fix: '', retryAfter: null,
  }, on);

  const element = h('div', { class: 'field secret-field' },
    first.label,
    input,
    second?.label,
    again,
    h('div', { class: 'btns' }, toggle),
    hint && h('span', { class: 'hint' }, hint),
    problem);

  return {
    element,

    /** Puts the focus in the first field. */
    focus: () => input.focus(),

    /**
     * The typed secret, once: the fields are empty after. Gives null, and
     * says why under the field, when it is empty and not optional or the
     * second does not say the same; what was typed stays then, for the
     * person to put right.
     */
    take() {
      for (const one of inputs) clearError(problem, one);
      const value = input.value;
      if (value === '' && !optional) {
        refuse(`Type the ${lowerFirst(label)}`, 'The field is empty.', input);
        return null;
      }
      if (again && again.value !== value) {
        refuse('The two are not the same', `Type the ${lowerFirst(label)} again exactly as above.`, again);
        return null;
      }
      clear();
      return value;
    },

    /** Shows a refusal of the server, which names the field, under it. */
    fail: (error) => showError(problem, error, input),

    clear,
  };
}
