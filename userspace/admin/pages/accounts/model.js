// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Accounts pages decide (design 7), as plain values so that a test
// can hold each choice: which view an address and a caller give, the group
// chips, the order of the list, the sentence about the sign-in rules, who may
// end what, the guard of a key that is an account's only way in (7.6), and the
// calls that the pages make for a change to the person's own account or to
// another's. What is typed is checked in typed-model.js, which the dialogs
// read when one is opened.

import {
  canWords, isDisabled, lastWords,
} from '../../lib/account-model.js';
import { counted } from '../../lib/size-words.js';

/** The sentence that goes with what the console never does to the account in use (ADM-136, design 7.6): the words, and the command in the code style. */
export const SELF = Object.freeze({
  words: 'Do this from another administrator\'s account, or at a terminal with',
  command: 'elevate user …',
});

// ---- who looks, and at what ---------------------------------------------------

/** The account the address names: `/accounts` is the list and gives null, `/accounts/chase` gives "chase". */
export function nameOf(rest) {
  const first = rest.split('/')[0];
  if (first === '') return null;
  try {
    return decodeURIComponent(first);
  } catch {
    return first;
  }
}

/** The address of the page of the account `name`. */
export const accountHref = (name) => `/accounts/${encodeURIComponent(name)}`;

/**
 * What the person looking may do: `reader` (not an administrator), `locked`
 * (an administrator whose console is locked) or `open`.
 */
export const standingOf = (session, unlocked) => {
  if (!session.admin) return 'reader';
  return unlocked ? 'open' : 'locked';
};

/**
 * Which view of one account the person gets: `own` for their own account,
 * whatever their standing (every call it makes is theirs to make);
 * `full` for another account from an unlocked administrator; `locked` and
 * `reader` for another account from an administrator who has not unlocked
 * and from a reader, who see the account's row and no more (design 7.8).
 */
export function viewOf({ own, standing }) {
  if (own) return 'own';
  return standing === 'open' ? 'full' : standing;
}

// ---- the list ------------------------------------------------------------------

/** The groups that rank first in the line of chips: the ones that say what an account can do. */
const FIRST_GROUPS = ['admins', 'console'];

/**
 * The line of groups: `[{name, count}]` with each group's number of
 * accounts. An account's own group, which holds it alone and is named as it
 * is, says nothing a person needs, and is left out.
 */
export function groupChips(accounts) {
  const members = new Map();
  for (const account of accounts) {
    for (const group of account.groups ?? []) members.set(group, [...(members.get(group) ?? []), account.name]);
  }
  const names = new Set(accounts.map((account) => account.name));
  const rank = (name) => (FIRST_GROUPS.includes(name) ? FIRST_GROUPS.indexOf(name) : FIRST_GROUPS.length);
  return [...members]
    .filter(([group, who]) => !(names.has(group) && who.length === 1 && who[0] === group))
    .map(([name, who]) => ({ name, count: who.length }))
    .sort((a, b) => rank(a.name) - rank(b.name) || (a.name < b.name ? -1 : 1));
}

/** The accounts in the order of the list: as the command gives them, the disabled ones last. */
export const ordered = (accounts) => [...accounts.filter((account) => !isDisabled(account)), ...accounts.filter(isDisabled)];

/**
 * The rows of the table, in order: `{account, own, disabled, can, last}`.
 * `last` is a `Map` of name to `lastlog`'s entry, or null where the person
 * may not see when each was last active; `keys` is a `Map` of name to how
 * many keys an account with no password has, which says whether it can sign
 * in at all; `self` is the name of the person looking.
 */
export function listRows({
  accounts, last = null, keys = new Map(), self, zone, now = Date.now(),
}) {
  return ordered(accounts).map((account) => {
    const own = account.name === self;
    return {
      account,
      own,
      disabled: isDisabled(account),
      can: canWords(account, keys.get(account.name) ?? null),
      last: last === null ? null : lastWords(last.get(account.name), zone, { now, own }),
    };
  });
}

const SECOND_STEP_RULES = Object.freeze({
  administrators: 'Administrators give a second step.',
  everyone: 'Everyone gives a second step.',
  'if-set': 'Anyone who has a second step gives it.',
});

/**
 * The console's sign-in rules in a sentence, from `admin settings --json`:
 * "Administrators give a second step. A session lasts 12 hours." Null when
 * the answer does not say.
 */
export function ruleWords(settings) {
  const rules = settings?.sign_in;
  if (!rules || !Number.isFinite(rules.session_hours)) return null;
  return [SECOND_STEP_RULES[rules.second_step], `A session lasts ${counted(rules.session_hours, 'hour')}.`].filter(Boolean).join(' ');
}

/** How many keys authd can use `shown`, an answer of `user show`, lists: those without options. Null when there is no answer. */
export const keyCount = (shown) => (Array.isArray(shown?.keys) ? shown.keys.filter((key) => key.options !== true).length : null);

// ---- sessions --------------------------------------------------------------------

/**
 * Whether End is offered on `row` (a row of `signedIn`) in `view`. The
 * session in use is Sign out's; the person ends their own other browsers and
 * terminals, and an unlocked administrator ends anything of another account.
 * A session at the machine is ended by `user end`, which is an
 * administrator's and never of the account in use (ADM-136).
 */
export function mayEnd(row, view) {
  if (row.this) return false;
  if (view === 'full') return true;
  return view === 'own' && row.scope !== 'machine';
}

/**
 * What End asks for on `row`: `{path}` for a console session or a terminal,
 * which are routes of admind's own, and `{action, args}` for a session at
 * the machine, which `accounts.end` ends.
 */
export function endCall(name, row) {
  if (row.scope === 'console') return { path: `/api/1/accounts/${encodeURIComponent(name)}/sessions/${encodeURIComponent(row.id)}/end` };
  if (row.scope === 'terminal') return { path: `/api/1/terminals/${encodeURIComponent(row.id)}/end` };
  return { action: 'accounts.end', args: { name, session: row.id } };
}

// ---- keys ------------------------------------------------------------------------

/**
 * Whether removing `key` leaves `account` no way to sign in (design 7.6): it
 * is the last key authd can use, and the account has no password or SSH
 * passwords are off (`sshPasswords` false; null when not known), and no
 * right to the console.
 */
export function onlyWayIn({ account, keys, key, sshPasswords = null }) {
  if (key.options === true) return false;
  if (keys.filter((one) => one.options !== true).length !== 1) return false;
  if (account.admin || account.console) return false;
  return account.password === 'none' || sshPasswords === false;
}

/** What "Unlock asks" says for an account whose second steps are `steps`: the proof the unlock dialog wants. */
export function unlockAsks(steps) {
  const proofs = [
    Array.isArray(steps?.passkeys) && steps.passkeys.length > 0 ? 'a tap on a passkey' : null,
    steps?.app ? 'the code from the app' : null,
  ].filter(Boolean);
  return proofs.length > 0 ? `for ${proofs.join(', or ')}` : 'for your password';
}

// ---- the calls of a change, for the person's own account or another's ---------------------

/** The call that gives the account `name` the full name `fullName`: the person's own, or an administrator's of another's. */
export const nameCall = ({ own, name, fullName }) => (own
  ? { action: 'account.name', args: { full_name: fullName } }
  : { action: 'accounts.set', args: { name, full_name: fullName } });

/** The call that sets one or more of `admin` and `console` on the account `name`. */
export const rightsCall = ({ name, ...rights }) => ({ action: 'accounts.set', args: { name, ...rights } });

/**
 * The call that changes a password: the person's own with the one they
 * have (`current`) and the new one, or an administrator's of another's with
 * the new one alone. This and `addArgs` are the two places a typed password
 * is put in a call's arguments.
 */
export const passwordCall = ({
  own, name, current = '', password,
}) => (own
  ? { action: 'account.password', args: { current, new: password } }
  : { action: 'accounts.password', args: { name, password } });

/** The arguments of `POST /api/1/show` for `passwordCall`: a required secret is not sent, and the command says that it is typed when asked. */
export const showPasswordArgs = ({ own, name }) => ({
  action: own ? 'account.password' : 'accounts.password',
  args: own ? {} : { name },
});

/** The call that adds the SSH key `line` to the account `name`. */
export const keyAddCall = ({ own, name, line }) => (own
  ? { action: 'account.key-add', args: { line } }
  : { action: 'accounts.key-add', args: { name, line } });

/** The call that removes the key with `fingerprint` from the account `name`. */
export const keyRemoveCall = ({ own, name, fingerprint }) => (own
  ? { action: 'account.key-remove', args: { fingerprint } }
  : { action: 'accounts.key-remove', args: { name, fingerprint } });

/** The call that revokes the token `token` of the account `name`, another's: the person's own tokens go by C2a's route. */
export const tokenRevokeCall = ({ name, token }) => ({ action: 'accounts.token-revoke', args: { name, token } });

/** The call that removes the second steps of the account `name`. */
export const stepsResetCall = ({ name }) => ({ action: 'accounts.steps-reset', args: { name } });

/** The calls that disable and enable the account `name`. */
export const disableCall = ({ name }) => ({ action: 'accounts.disable', args: { name } });
export const enableCall = ({ name }) => ({ action: 'accounts.enable', args: { name } });

/** The call that removes the account `name` and does `home` with its home folder. */
export const removeCall = ({ name, home }) => ({ action: 'accounts.remove', args: { name, home } });
