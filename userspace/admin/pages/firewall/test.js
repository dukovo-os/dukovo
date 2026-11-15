// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Test a connection (design 10.3): the protocol, port and address of a
// connection that is not made yet, and optionally the card it would come by,
// and the answer of `firewall test` in words: which rule decides, or that
// none does. Anyone may ask: it reads the settings file and changes nothing.

import {
  clearError, errorSlot, fill, h, icon,
} from '../../dom.js';
import { blankTest, checkTest, testWords } from './report-model.js';

/** Puts `why` under `input`, in `slot`, and marks the input. */
function showWhy(slot, input, why) {
  slot.replaceChildren(icon('warn'), h('span', {}, why));
  input.classList.add('bad');
  input.setAttribute('aria-invalid', 'true');
  input.setAttribute('aria-describedby', slot.id);
}

/**
 * The card. `rules()` gives the page's rules as rules, for the words of the
 * rules a test passes over. `set({cards})` gives the names of the cards, for
 * the choice of On. Gives `{element, set}`.
 */
export function makeTest(view, { rules }) {
  const form = blankTest();
  form.from = view.session.from ?? '';
  const protocol = h('select', { id: 'fw-test-protocol', class: 'input' }, h('option', { value: 'tcp' }, 'TCP'), h('option', { value: 'udp' }, 'UDP'));
  const port = h('input', { id: 'fw-test-port', class: 'input mono', type: 'text', inputmode: 'numeric', autocomplete: 'off', spellcheck: 'false' });
  const portError = errorSlot('fw-test-port-error');
  const from = h('input', { id: 'fw-test-from', class: 'input mono', type: 'text', value: form.from, autocomplete: 'off', spellcheck: 'false' });
  const fromError = errorSlot('fw-test-from-error');
  const on = h('select', { id: 'fw-test-on', class: 'input' }, h('option', { value: '' }, 'Any card'));
  const go = h('button', { class: 'btn default', type: 'submit' }, 'Test');
  const problems = h('div', { class: 'fw-problems' });
  const answer = h('div', { class: 'fw-answer', role: 'status' });
  const element = h('form', { class: 'card fw-test', 'aria-labelledby': 'fw-test-h', novalidate: true },
    h('h2', { id: 'fw-test-h' }, 'Test a connection'),
    h('p', { class: 'note' }, 'Ask which rule would decide a new connection, before anyone makes it.'),
    h('div', { class: 'fw-two' },
      h('div', { class: 'field' }, h('label', { for: protocol.id }, 'Protocol'), protocol),
      h('div', { class: 'field' }, h('label', { for: port.id }, 'Port'), port, portError)),
    h('div', { class: 'fw-two' },
      h('div', { class: 'field' }, h('label', { for: from.id }, 'From address'), from, fromError),
      h('div', { class: 'field' }, h('label', { for: on.id }, 'On'), on)),
    problems, h('div', { class: 'btns' }, go), answer);

  protocol.addEventListener('change', () => { form.protocol = protocol.value; });
  port.addEventListener('input', () => {
    form.port = port.value;
    clearError(portError, port);
  });
  from.addEventListener('input', () => {
    form.from = from.value;
    clearError(fromError, from);
  });
  on.addEventListener('change', () => { form.on = on.value; });

  element.addEventListener('submit', async (event) => {
    event.preventDefault();
    clearError(portError, port);
    clearError(fromError, from);
    problems.replaceChildren();
    answer.replaceChildren();
    const made = checkTest(form);
    if (!made.args) {
      const place = { port: [portError, port], from: [fromError, from] }[made.field];
      if (place) {
        showWhy(place[0], place[1], made.why);
        place[1].focus();
      }
      return;
    }
    const result = await view.api.run('firewall.test', made.args, {
      button: go, words: { unlock: 'test a connection', busy: 'Testing…' }, where: { slot: problems, fields: { port, from, on } },
    });
    if (!result.ok) return;
    const lines = testWords(result.result, made.args.port, rules());
    fill(answer, lines.map((line, at) => h('p', { class: at === 0 ? 'fw-verdict' : 'muted' }, line)));
  });

  return {
    element,
    /** Gives the names of the cards, keeping the one chosen. */
    set({ cards }) {
      const names = [...new Set(cards)];
      on.replaceChildren(h('option', { value: '' }, 'Any card'), names.map((name) => h('option', { value: name }, name)));
      on.value = names.includes(form.on) ? form.on : '';
      form.on = on.value;
    },
  };
}
