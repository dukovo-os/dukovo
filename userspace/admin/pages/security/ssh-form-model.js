// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the SSH form asks (design 8.6 and 9.1) as plain values: the form's
// fields, the change they make, the calls that make it, with the window of a
// try, and what the change does. They are made from `ssh-server show --json`
// (design 11.8). Nothing here touches the page.

import { WINDOWS } from '../../lib/trial-model.js';
import { narrows, nobody } from './model.js';
import { whoWords } from './ssh-model.js';

const names = (list) => (Array.isArray(list) ? list : []);

/** The windows of an SSH try, 2, 5 and 10 minutes: a new connection takes a while to open, so 60 seconds is too short. */
export const SSH_WINDOWS = Object.freeze(WINDOWS.filter((one) => [120, 300, 600].includes(one.seconds)));

/** The window that is chosen. */
export const SSH_WINDOW = 300;

/** The most names `ssh.set` takes in a list. */
export const NAMES_MOST = 32;

/** Who may sign in, as the form's choice. */
export const WHO = Object.freeze(['everyone', 'users', 'groups', 'both']);

/**
 * The form's fields for the settings `show`, `{on, passwords, keys, who,
 * users, groups}`. `preset` is `keys-only` (passwords off, keys on) or `who`
 * (the accounts' choice, when everyone may sign in now).
 */
export function formOf(show, preset = null) {
  const users = [...names(show?.allow_users)];
  const groups = [...names(show?.allow_groups)];
  const form = {
    on: show?.on !== false,
    passwords: show?.passwords === true,
    keys: show?.keys !== false,
    users,
    groups,
    who: users.length > 0 && groups.length > 0 ? 'both' : users.length > 0 ? 'users' : groups.length > 0 ? 'groups' : 'everyone',
  };
  if (preset === 'keys-only') Object.assign(form, { passwords: false, keys: true });
  if (preset === 'who' && form.who === 'everyone') form.who = 'users';
  return form;
}

/** The settings the form holds, as `ssh.show` has them: a list that is empty is everyone. */
export function settingsOf(form) {
  const users = form.who === 'users' || form.who === 'both' ? form.users : [];
  const groups = form.who === 'groups' || form.who === 'both' ? form.groups : [];
  return {
    on: form.on, passwords: form.passwords, keys: form.keys, allow_users: users, allow_groups: groups,
  };
}

/** The settings of `show`, as `settingsOf` gives the form's. */
export const settingsOfShow = (show) => settingsOf(formOf(show));

/** What is wrong with the form, as a sentence, or empty text: no way in, or a list with nobody in it. */
export function problemOf(form) {
  if (!form.on) return '';
  if (nobody(form)) return 'Nobody could sign in over SSH. Turn on passwords, keys or both.';
  if ((form.who === 'users' || form.who === 'both') && form.users.length === 0) return 'Choose at least one account, or let everyone sign in.';
  if ((form.who === 'groups' || form.who === 'both') && form.groups.length === 0) return 'Choose at least one group, or let everyone sign in.';
  if (form.users.length > NAMES_MOST || form.groups.length > NAMES_MOST) return `A list holds at most ${NAMES_MOST} names.`;
  return '';
}

/** Whether two lists have the same names, in whatever order. */
const sameNames = (a, b) => a.length === b.length && a.every((name) => b.includes(name));

/** The arguments of `ssh.set` for what changes from `before` to `after` (both `{on, passwords, keys, allow_users, allow_groups}`): only what changed. */
export function setArgs(before, after) {
  const args = {};
  if (before.passwords !== after.passwords) args.passwords = after.passwords ? 'yes' : 'no';
  if (before.keys !== after.keys) args.keys = after.keys ? 'yes' : 'no';
  if (!sameNames(before.allow_users, after.allow_users)) args.allow_users = after.allow_users.length > 0 ? after.allow_users.join(' ') : 'everyone';
  if (!sameNames(before.allow_groups, after.allow_groups)) args.allow_groups = after.allow_groups.length > 0 ? after.allow_groups.join(' ') : 'everyone';
  return args;
}

/**
 * The calls that make the change from `before` to `after`, in order, `[{action,
 * args}]`; with `seconds` the call that can take a way in from someone is a
 * try. Turning SSH off is `ssh.off`; turning it on first is `ssh.on`, which
 * takes nothing away and is never tried. The calls are none when nothing
 * changes.
 */
export function callsOf(before, after, seconds = null) {
  const tried = seconds === null ? {} : { try: seconds };
  if (!after.on) return before.on ? [{ action: 'ssh.off', args: tried }] : [];
  const calls = before.on ? [] : [{ action: 'ssh.on', args: {} }];
  const args = setArgs(before, after);
  if (Object.keys(args).length > 0) calls.push({ action: 'ssh.set', args: { ...args, ...tried } });
  return calls;
}

/**
 * What the change `before` to `after` does, in a few sentences for the form,
 * and whether it is `tried` first. A change that narrows who signs in is
 * only offered as a try; one that does not is applied or tried.
 */
export function effectOf(before, after) {
  const lines = [];
  if (before.on && !after.on) lines.push('SSH stops. Nobody can sign in over SSH.');
  if (after.on) {
    if (before.passwords && !after.passwords) lines.push('Passwords stop signing in over SSH.');
    if (before.keys && !after.keys) lines.push('Keys stop signing in over SSH.');
    if (!before.passwords && after.passwords) lines.push('Passwords sign in over SSH.');
    if (!before.keys && after.keys) lines.push('Keys sign in over SSH.');
    const who = whoWords(after.allow_users, after.allow_groups);
    if (who !== whoWords(before.allow_users, before.allow_groups)) lines.push(who === null ? 'Every account may sign in over SSH.' : `Over SSH, ${who} may sign in.`);
  }
  return { lines, narrows: narrows(before, after), changes: callsOf(before, after).length > 0 };
}
