// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The two questions of the Checks card: "Add a check…", a web address that
// answers or a service that runs, required or wanted, and "Add a service…",
// one more service that must run again after an update. Read when first
// asked for.

import { h, uniqueId } from '../../dom.js';
import { openFormDialog } from '../../lib/form-dialog.js';
import { addArgs, CONSOLE_CHECK } from './check-form-model.js';

/** A labelled field: `{field, input}`. */
function field(label, input, hint = null) {
  if (!input.id) input.setAttribute('id', uniqueId('check-field'));
  return h('div', { class: 'field' }, h('label', { for: input.id }, label), input, hint && h('span', { class: 'hint' }, hint));
}

/** Opens "Add a check…". Gives whether one was added. */
export function openAddCheck(ctx) {
  const { view } = ctx;
  const what = h('select', { class: 'select' }, h('option', { value: 'url' }, 'A web address answers'), h('option', { value: 'service' }, 'A service is running'));
  const target = h('input', { class: 'input', type: 'text', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off' });
  const name = h('input', { class: 'input', type: 'text', autocomplete: 'off', maxlength: '64' });
  const required = h('select', { class: 'select' }, h('option', { value: 'no' }, 'Wanted: you get an alert and the update stays'), h('option', { value: 'yes' }, 'Required: the update is taken back'));
  const timeout = h('input', { class: 'input', type: 'number', min: '1', max: '600', value: '30' });
  const insecure = h('input', { type: 'checkbox' });
  const errors = h('div', { class: 'msg', role: 'alert' });
  const form = () => ({
    name: name.value, what: what.value, target: target.value, insecure: insecure.checked === true, required: required.value === 'yes', timeout: timeout.value,
  });
  const ready = h('button', { class: 'btn small', type: 'button' }, 'This console answers');
  ready.addEventListener('click', () => {
    what.value = CONSOLE_CHECK.what;
    target.value = CONSOLE_CHECK.target;
    name.value = CONSOLE_CHECK.name;
    insecure.checked = true;
  });
  return openFormDialog(view, {
    title: 'Add a check',
    icon: 'check',
    lead: 'A check runs after an update starts. Required ones take the update back when they fail; wanted ones only send an alert.',
    fields: [
      field('What', what), field('Address or service', target), field('Name', name), field('If it fails', required), field('Seconds it may take', timeout),
      h('div', { class: 'check' }, insecure, h('label', {}, 'Accept a certificate that is not trusted (for a web address on this machine)')),
      h('div', { class: 'btns' }, ready), errors,
    ],
    label: 'Add the check',
    command: () => addArgs(form()).args && { action: 'updates.check-add', args: addArgs(form()).args },
    submit: async (button, slot) => {
      const made = addArgs(form());
      if (made.problem) {
        errors.textContent = made.problem;
        return false;
      }
      errors.textContent = '';
      return (await view.api.run('updates.check-add', made.args, { button, where: { slot }, words: { unlock: 'add a check', done: 'The check is added.' } })).ok;
    },
  });
}

/** Opens "Add a service…", with the services this machine runs that are not checked yet. Gives whether one was added. */
export async function openAddService(ctx) {
  const { view, data } = ctx;
  const have = new Set(data.checks?.essential ?? []);
  const answer = await view.api.get('/api/1/services').catch(() => null);
  const names = (Array.isArray(answer?.services) ? answer.services : []).map((one) => one?.name).filter((one) => typeof one === 'string' && one !== '' && !have.has(one));
  if (names.length === 0) {
    view.notice('Every service is already one that must run again.', { kind: 'warn' });
    return false;
  }
  const service = h('select', { class: 'select' }, ...names.map((one) => h('option', { value: one }, one)));
  return openFormDialog(view, {
    title: 'Add a service',
    icon: 'services',
    lead: 'After an update, this service has to run again, or the update is taken back.',
    fields: [field('Service', service)],
    label: 'Add the service',
    command: () => ({ action: 'updates.essential', args: { service: service.value || names[0], on: true } }),
    submit: async (button, slot) => (await view.api.run('updates.essential', { service: service.value || names[0], on: true }, {
      button, where: { slot }, words: { unlock: 'add a service to the checks', done: 'The service is added.' },
    })).ok,
  });
}
