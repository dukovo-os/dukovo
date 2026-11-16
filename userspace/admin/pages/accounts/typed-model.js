// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What is typed in the Accounts pages' dialogs (design 7.4, 7.5 and 7.6),
// checked before the command makes the same checks again: a name, a full name
// and a key line as authd reads it, and the arguments of Add an account. A
// secret is put in the arguments in `addArgs` and in `passwordCall` of
// model.js and nowhere else. Read when the first dialog is opened.

// ---- keys ------------------------------------------------------------------------

/** The key types authd takes (`accounts::request::SSH_KEY_ALGORITHMS`). */
const KEY_TYPES = ['ssh-ed25519', 'ecdsa-sha2-nistp256', 'ecdsa-sha2-nistp384'];

/** The longest line of a key the action takes, in bytes. */
export const KEY_LINE_MAX = 16 * 1024;

/** Whether `blob`, the base64 of a key, is one of the kind `type` says: its first string is the type, as SSH carries it. */
function blobHolds(type, blob) {
  let bytes;
  try {
    bytes = Uint8Array.from(atob(blob), (character) => character.charCodeAt(0));
  } catch {
    return false;
  }
  if (bytes.length < 4) return false;
  const length = new DataView(bytes.buffer).getUint32(0);
  return new TextDecoder().decode(bytes.subarray(4, 4 + length)) === type && bytes.length > 4 + length;
}

/**
 * Why `text` is not a key line that authd takes (`sshkeys.rs`), in words,
 * or an empty text when it is one: one line as `ssh-keygen` writes it in a
 * `.pub` file, Ed25519 or ECDSA on P-256 or P-384, and no options before it.
 */
export function keyLineProblem(text) {
  const line = text.trim();
  if (line === '') return 'Paste the key: the line that ssh-keygen wrote in the .pub file.';
  if (/[\r\n]/.test(line)) return 'Paste one key, on one line.';
  if (new TextEncoder().encode(line).length > KEY_LINE_MAX) return 'That is longer than a key can be.';
  const [type, blob] = line.split(/\s+/);
  if (!KEY_TYPES.includes(type)) {
    if (/^(?:ssh-|ecdsa-sha2-|sk-|rsa-sha2-)/.test(type)) return `Dukovo takes Ed25519 and ECDSA keys on P-256 and P-384, not ${type}.`;
    return 'The line does not start with a key type. A line with options before the key, such as command= or from=, is not taken: paste the key alone.';
  }
  return blob !== undefined && blobHolds(type, blob) ? '' : 'The key is not whole: copy the entire line.';
}

/** The longest name of an account, in characters (`accounts::NAME_MAX`). */
export const NAME_MAX = 32;

/** The name that sign-ins of the help sessions use (`accounts::request::HELP_NAME`), which is no person's. */
const HELP_NAME = 'help';

/**
 * Why `name` cannot be a new account's name, in words, or an empty text
 * when it can: the rule of `accounts::check_name`, and that `taken` (the
 * names of the accounts) has not got it already.
 */
export function nameProblem(name, taken = []) {
  if (name === '') return 'A name needs a letter at least.';
  if (name.length > NAME_MAX) return `A name has at most ${NAME_MAX} characters.`;
  if (!/^[a-z_][a-z0-9_.-]*$/.test(name)) return 'Use lowercase letters, digits, _, - and ., starting with a letter.';
  if (name === HELP_NAME) return `${HELP_NAME} is kept for the helpers who sign in to help sessions: choose another name.`;
  return taken.includes(name) ? `An account is already called ${name}.` : '';
}

/** Why `text` cannot be a full name, or an empty text: no colon or line break, and 256 bytes at most (`accounts::check_full_name`). */
export function fullNameProblem(text) {
  if (/[:\r\n]/.test(text)) return 'A full name cannot hold a colon or a line break.';
  return new TextEncoder().encode(text).length > 256 ? 'A full name has at most 256 bytes.' : '';
}

/**
 * The arguments of `accounts.add`: the name, and the full name, the right to
 * administer and the password when they are given. This is the one place a
 * typed password is put in a call's arguments.
 */
export function addArgs({
  name, fullName = '', admin = false, password = '',
}) {
  const args = { name };
  if (fullName !== '') args.full_name = fullName;
  if (admin) args.admin = true;
  if (password !== '') args.password = password;
  return args;
}

/**
 * The arguments of `POST /api/1/show` for the call that `addArgs` builds,
 * with the password as an empty text when one is typed: the command says
 * "then the password, typed when asked", and no password is ever sent there.
 */
export function showAddArgs({ hasPassword, ...rest }) {
  const args = addArgs(rest);
  return hasPassword ? { ...args, password: '' } : args;
}

/** What becomes of the home folder of a removed account: the values of `accounts.remove`'s `home`, in the order they are offered, each with its label and what it does. */
export const HOME_CHOICES = Object.freeze([
  {
    value: 'keep',
    label: (name) => `Keep it in /users/${name}`,
    words: 'The files stay where they are, and only administrators reach them. A new account of the same name is told so.',
  },
  {
    value: 'archive',
    label: () => 'Archive it',
    words: 'The home is packed into a file in /users/.removed, which only root reads, and then deleted.',
  },
  {
    value: 'delete',
    label: () => 'Delete it',
    words: 'The home and every file in it is deleted. It comes back only from a snapshot or a backup.',
  },
]);

