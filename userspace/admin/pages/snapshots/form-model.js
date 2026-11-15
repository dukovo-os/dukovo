// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the forms of the Snapshots and backups page may send (design 5.5 and
// 5.8), as plain values: what Add a backup place checks and makes of what it
// was given, the recovery key and the SSH key that an answer holds, and what
// Take a snapshot now checks. The server checks all of it again, as backup
// and snap do. The forms are read when a button first asks for one.

import { placesWords } from '../../lib/snapshot-model.js';
import { hostOf } from './model.js';

// ---- Add a backup place --------------------------------------------------------------------------------------

/** The most characters of a backup place's name. */
export const NAME_MOST = 32;

/** The words that say why `name` cannot name a backup place, or empty text. `taken` lists the names in use. */
export function nameProblem(name, taken = []) {
  if (name === '') return 'Give it a name';
  if (name.length > NAME_MOST) return `Use at most ${NAME_MOST} characters`;
  if (!/^[A-Za-z0-9_-]+$/.test(name)) return 'Use letters, digits, - and _ only';
  if (name.startsWith('-')) return 'The name cannot start with -';
  if (taken.includes(name)) return `A backup place is already called ${name}`;
  return '';
}

/** `text` made into a name that `nameProblem` accepts, or empty text when nothing of it is left. */
function nameFrom(text) {
  return text.replace(/[^A-Za-z0-9_-]+/g, '-').replace(/^-+|-+$/g, '').slice(0, NAME_MOST);
}

/**
 * The name to suggest, `usb-LABEL` for a disk and the server's own name for
 * an SSH server (`user@host:/folder` or `ssh://…`); empty text when there is
 * nothing to make it from. A host that is an address makes no name.
 */
export function suggestName({ where, volume = null, address = '' }) {
  if (where === 'disk') return volume ? nameFrom(`usb-${volume.label}`) : '';
  if (where !== 'ssh') return '';
  const host = hostOf(address.trim());
  if (host === '' || /^[0-9.]+$/.test(host) || host.includes(':')) return '';
  return nameFrom(host.split('.')[0]);
}

/**
 * The address a typed SSH server gives, `{to}` as backup(8) takes it
 * (`ssh://user@host/folder`), or `{error}`. `user@host:/folder` is what a
 * person types, as `scp` does; `ssh://…` is taken as it is, which is the way
 * to write another port.
 */
export function sshAddress(text) {
  const typed = text.trim();
  if (typed.startsWith('ssh://')) {
    return /^ssh:\/\/[^@/\s]+@[^/\s]+\/\S*$/.test(typed) ? { to: typed } : { error: 'Write it as ssh://user@host:2222/folder' };
  }
  const found = /^([^@\s/:]+)@(\[[0-9A-Fa-f:]+\]|[^\s/:@]+):(\/\S*|~\/\S*)$/.exec(typed);
  if (!found) return { error: 'Write it as user@host:/folder, such as backup@192.168.1.20:/srv/dukovo' };
  const [, user, host, path] = found;
  return { to: `ssh://${user}@${host}${path.startsWith('~/') ? `/${path}` : path}` };
}

/** What each choice of "How often" and "Check by restoring" sends, with the words of its label. */
export const EVERY = Object.freeze([
  { value: 'day', label: 'Every day' },
  { value: 'week', label: 'Every week' },
  { value: 'hand', label: 'By hand' },
]);
export const VERIFY = Object.freeze([
  { value: 'week', label: 'Every week' },
  { value: 'month', label: 'Every month' },
  { value: 'never', label: 'Never' },
]);

/** What the page tells of S3 storage, which only the command adds. */
export const S3_WORDS = 'S3 storage is added at a terminal for now, because the command reads its keys from the environment.';

/**
 * Checks what Add a backup place was given and makes the action's arguments:
 * `{args}` or `{errors: {name, to}}`, each a sentence for under its field.
 * `values` is `{where, volume, address, name, every, verify, format}`;
 * `taken` the names in use. The server checks again, as backup does.
 */
export function addArgs(values, taken = []) {
  const errors = {};
  const name = values.name.trim();
  const bad = nameProblem(name, taken);
  if (bad) errors.name = bad;
  let to = null;
  if (values.where === 'disk') {
    if (values.volume) to = values.volume.to;
    else errors.to = 'Choose a disk';
  } else if (values.where === 'ssh') {
    const found = sshAddress(values.address);
    if (found.error) errors.to = found.error;
    else to = found.to;
  } else {
    errors.to = S3_WORDS;
  }
  if (Object.keys(errors).length > 0) return { errors };
  const args = { name, to, every: values.every, verify: values.verify };
  if (values.format === 'restic') args.format = 'restic';
  return { args };
}

/** The recovery key that an answer of `backups.add` or `backups.key` holds, or null: a line that starts AGE-SECRET-KEY-1. */
export function recoveryKeyOf(answer) {
  const walk = (value, depth) => {
    if (typeof value === 'string') return /AGE-SECRET-KEY-1[0-9A-Z]+/.exec(value)?.[0] ?? null;
    if (depth > 3 || value === null || typeof value !== 'object') return null;
    for (const inner of Object.values(value)) {
      const found = walk(inner, depth + 1);
      if (found) return found;
    }
    return null;
  };
  return walk(answer, 0);
}

/** The public key that `backups.ssh-key` answers with, a line such as `ssh-ed25519 AAAA… host`, or null. */
export function sshKeyOf(answer) {
  const walk = (value, depth) => {
    if (typeof value === 'string') return /(?:ssh-(?:ed25519|rsa)|ecdsa-sha2-nistp\d+|sk-[a-z0-9@.-]+)[ \t]+[A-Za-z0-9+/]+={0,2}(?:[ \t]+[^\r\n]+)?/.exec(value)?.[0].trim() ?? null;
    if (depth > 3 || value === null || typeof value !== 'object') return null;
    for (const inner of Object.values(value)) {
      const found = walk(inner, depth + 1);
      if (found) return found;
    }
    return null;
  };
  return walk(answer, 0);
}

// ---- Take a snapshot now ----------------------------------------------------------------------------------------

/** Whether `name` is a time a snapshot is named by, which snap gives itself: `YYYY-MM-DD-HHMM`, with `.N` for N from 2. */
const isTimed = (name) => /^\d{4}-\d{2}-\d{2}-\d{4}(?:\.(?:[2-9]|[1-9]\d+))?$/.test(name);

/** The words that say why `name` cannot be given to a snapshot, as `keep::may_name` checks it, or empty text. The name is optional. */
export function snapshotNameProblem(name) {
  if (name === '') return '';
  if (name.length > 255) return 'Use at most 255 characters';
  if (name.startsWith('.')) return 'The name cannot start with a dot';
  if (name.includes('/')) return 'The name cannot hold a /';
  if (/\0/.test(name)) return 'The name cannot hold a NUL character';
  if (isTimed(name)) return 'A name like that is a time, which snap gives itself';
  return '';
}

/** The sentences that tell which snapshots of Take a snapshot now were made, from the results `[{place, ok}]`. */
export function takenWords(results) {
  const made = results.filter((one) => one.ok).map((one) => one.place);
  const missed = results.filter((one) => !one.ok).map((one) => one.place);
  return [
    made.length > 0 && `Made a snapshot of ${placesWords(made)}.`,
    missed.length > 0 && `No snapshot was made of ${placesWords(missed)}.`,
  ].filter(Boolean);
}
