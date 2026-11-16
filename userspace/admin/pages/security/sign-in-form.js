// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The form that changes the sign-in rules (design 8.4): the second step's rule
// and the two lengths of a session, with what follows from the change said
// under them before it is sent. Read when the form is first opened.

import { fill, h } from '../../dom.js';
import { openForm } from './dialog.js';
import { choiceField, textField } from './fields.js';
import {
  HOURS, MINUTES, ruleChange, rulesProblems,
} from './rules-form-model.js';
import { SECOND_STEPS } from './rules-model.js';

/** Opens the form for `rules`; `reread()` reads the rules again after a change. Resolves when the dialog closes. */
export async function openRulesForm(view, { rules, reread }) {
  const hours = textField({ label: 'Hours a session lasts after its sign-in', hint: `${HOURS.least} to ${HOURS.most}`, value: String(rules.session_hours) });
  const minutes = textField({ label: 'Minutes with no console page open before a session ends', hint: `${MINUTES.least} to ${MINUTES.most}`, value: String(rules.idle_minutes) });
  for (const field of [hours, minutes]) field.input.setAttribute('inputmode', 'numeric');
  const follows = h('div', { class: 'sec-follows', role: 'status' });
  const step = choiceField({
    label: 'Who gives a second step', value: rules.second_step, choices: SECOND_STEPS.map(({ value, label }) => ({ value, label })), onChange: draw,
  });
  const form = () => ({ second_step: step.value(), hours: hours.input.value, minutes: minutes.input.value });
  const save = { label: 'Save the rules', kind: 'default' };
  let dialog = null;
  /** Says what is wrong as it is typed, or else what the change does, under the fields; Save is on for a change that can be sent. */
  function draw() {
    const problems = rulesProblems(form());
    const wrong = Object.values(problems);
    for (const [field, problem] of [[hours, problems.hours], [minutes, problems.minutes]]) {
      if (problem) field.input.setAttribute('aria-invalid', 'true');
      else field.input.removeAttribute('aria-invalid');
    }
    fill(follows, ...(wrong.length > 0
      ? wrong.map((words) => h('p', { class: 'msg' }, words))
      : ruleChange(rules, form()).lines.map((words) => h('p', {}, words))));
    if (dialog) dialog.buttons[0].disabled = wrong.length > 0 || Object.keys(ruleChange(rules, form()).args).length === 0;
  }
  save.submit = async (button, slot) => {
    const problems = rulesProblems(form());
    if (problems.hours) hours.fault(problems.hours);
    if (problems.minutes) minutes.fault(problems.minutes);
    if (Object.keys(problems).length > 0) return false;
    const { args } = ruleChange(rules, form());
    const result = await view.api.run('security.sign-in-set', args, {
      button, where: { slot }, words: { unlock: 'change the sign-in rules', running: 'Changing the sign-in rules', done: 'The sign-in rules are changed.', unchanged: 'The sign-in rules were already so.' },
    });
    if (result.ok) reread();
    return result.ok;
  };
  dialog = openForm(view, {
    title: 'Change the sign-in rules',
    icon: 'lock',
    lead: 'These are the web console\'s rules and its API\'s. Not a trial: nobody\'s way in is taken away.',
    fields: [step.element, hours.element, minutes.element, follows],
    command: () => {
      const { args } = ruleChange(rules, form());
      return Object.keys(args).length > 0 && Object.keys(rulesProblems(form())).length === 0 ? { action: 'security.sign-in-set', args } : null;
    },
    buttons: [save],
  });
  for (const input of [hours.input, minutes.input]) input.addEventListener('input', draw);
  draw();
  await dialog.closed;
}
