// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The form that changes SSH's settings (design 8.6): the switch, the two ways
// to sign in, who may, and what the change does, with Try it for a change that
// takes a way in from someone and Apply as well for one that does not. Read when
// the form is first opened.

import { fill, h } from '../../dom.js';
import {
  callsOf, effectOf, formOf, problemOf, settingsOf, settingsOfShow, SSH_WINDOW, SSH_WINDOWS,
} from './ssh-form-model.js';

/** The words that a call of the form says it did. */
const doneWords = (action, tried) => (tried ? 'SSH\'s settings are being tried.' : { 'ssh.on': 'SSH is on.', 'ssh.off': 'SSH is off.' }[action] ?? 'SSH\'s settings are changed.');

/** What the form says of trying the change: whether it is tried first, or applied because sshd is not running. */
function tryWords(canTry, narrows) {
  if (!canTry) return 'SSH is not running now, so no connection over it can be cut off: the change is applied.';
  return narrows ? 'This takes a way in from someone, so it is tried first: it is undone unless you keep it.' : 'This only adds ways in. Apply it, or try it first.';
}

/** Runs the calls of a change one after the other; true when all were made. */
export async function runCalls(view, calls, button, slot) {
  for (const call of calls) {
    const result = await view.api.run(call.action, call.args, {
      button, where: { slot }, words: { unlock: 'change SSH\'s settings', running: 'Changing SSH\'s settings', done: doneWords(call.action, call.args.try !== undefined) },
    });
    if (!result.ok) return false;
  }
  return true;
}

/**
 * The form, for the settings `show` (`ssh.show`'s answer); `preset` is
 * `keys-only`, `who` or `off`. A change that is tried begins the trial of
 * `banner`; `read()` reads the settings again. Resolves when the dialog closes.
 */
export async function openSshForm(view, {
  show, preset = null, banner, read,
}) {
  const [{ openForm }, { choiceField, namesField, switchField }, accounts] = await Promise.all([
    import('./dialog.js'), import('./fields.js'), view.api.get('/api/1/accounts').catch(() => []),
  ]);
  const people = Array.isArray(accounts) ? accounts : [];
  const form = formOf(show, preset === 'off' ? null : preset);
  if (preset === 'off') form.on = false;
  const before = settingsOfShow(show);
  // A name the file lists that the machine no longer has stays in the list, so that a save does not drop it unseen.
  const known = (listed, found) => [...new Set([...found, ...listed])].sort();
  const accountNames = known(form.users, people.map((one) => one.name));
  const groupNames = known(form.groups, people.flatMap((one) => (Array.isArray(one.groups) ? one.groups : [])));
  const on = switchField({ label: 'SSH', hint: 'Off stops sshd: nobody signs in over SSH.', on: form.on });
  const passwords = switchField({ label: 'Passwords sign in', on: form.passwords });
  const keys = switchField({ label: 'Keys sign in', on: form.keys });
  const choices = [
    { value: 'everyone', label: 'Everyone', hint: 'Any account that may sign in.' },
    { value: 'users', label: 'These accounts' },
    { value: 'groups', label: 'These groups' },
    ...(show.allow_users?.length > 0 && show.allow_groups?.length > 0 ? [{ value: 'both', label: 'These accounts, and these groups' }] : []),
  ];
  const who = choiceField({ label: 'Who may sign in', choices, value: form.who, onChange: () => { form.who = who.value(); draw(); } });
  const users = namesField({ label: 'Accounts', names: accountNames, chosen: form.users, onChange: (list) => { form.users = list; draw(); } });
  const groups = namesField({ label: 'Groups', names: groupNames, chosen: form.groups, onChange: (list) => { form.groups = list; draw(); } });
  const follows = h('div', { class: 'sec-follows', role: 'status' });
  const windowSelect = h('select', { class: 'select', id: 'sec-ssh-window' }, ...SSH_WINDOWS.map((one) => h('option', { value: String(one.seconds), selected: one.seconds === SSH_WINDOW }, one.label)));
  const windowField = h('div', { class: 'field' }, h('label', { for: windowSelect.id }, 'Undo it after'), windowSelect, h('span', { class: 'hint' }, 'The change is undone then unless it is kept.'));
  const seconds = () => Number(windowSelect.value);
  // A try keeps the SSH connections sshd has, and ssh-server refuses one while
  // sshd is not running: then nobody is on SSH to be cut off, and the change
  // is applied.
  const canTry = show.on === true && show.running === true;
  const after = () => settingsOf({ ...form, on: on.input.checked, passwords: passwords.input.checked, keys: keys.input.checked });
  const calls = (trial) => callsOf(before, after(), trial && canTry ? seconds() : null);

  const apply = { label: 'Apply', kind: '', submit: (button, slot) => finish(button, slot, false) };
  const tryIt = { label: 'Try it', kind: 'default', submit: (button, slot) => finish(button, slot, true) };
  async function finish(button, slot, trial) {
    const planned = calls(trial);
    const made = await runCalls(view, planned, button, slot);
    // Only a call that is a try begins the trial; turning SSH on does not. The banner begins first, so that it knows the try is this page's.
    if (made && planned.some((call) => call.args.try !== undefined)) await banner.begin({ seconds: seconds() });
    await read();
    return made;
  }
  function draw() {
    const next = after();
    const merged = { ...form, ...next, who: form.who };
    const problem = problemOf(merged);
    const effect = effectOf(before, next);
    for (const field of [passwords, keys]) field.input.disabled = !on.input.checked;
    who.element.hidden = !on.input.checked;
    users.element.hidden = !on.input.checked || !['users', 'both'].includes(form.who);
    groups.element.hidden = !on.input.checked || !['groups', 'both'].includes(form.who);
    windowField.hidden = !canTry || !effect.changes || problem !== '';
    fill(follows,
      problem && h('p', { class: 'msg' }, problem),
      !problem && effect.lines.map((words) => h('p', {}, words)),
      !problem && effect.changes && h('p', { class: 'small muted' }, tryWords(canTry, effect.narrows)));
    if (dialog.buttons.length > 0) {
      dialog.buttons[0].hidden = canTry && effect.narrows;
      dialog.buttons[1].hidden = !canTry;
      for (const button of dialog.buttons) button.disabled = !effect.changes || problem !== '';
    }
  }
  const dialog = openForm(view, {
    title: 'Change SSH\'s settings',
    icon: 'key',
    lead: 'SSH signs in by these settings, not by the console\'s rules. Root never signs in over SSH.',
    fields: [on.element, passwords.element, keys.element, who.element, users.element, groups.element, windowField, follows],
    command: () => calls(false).filter((call) => call.action === 'ssh.set' || call.action === 'ssh.off')[0] ?? null,
    buttons: [apply, tryIt],
  });
  for (const field of [on, passwords, keys]) field.input.addEventListener('change', draw);
  draw();
  await dialog.closed;
}
