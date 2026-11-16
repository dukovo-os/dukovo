// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Answers of the accounts' reads, in the shapes of design 11.7 (`user list
// --json`, `user show --json`, `lastlog --json`) and 13.3 (the sessions
// route), for the tests of the Accounts pages. The times are those of
// snapshots-fixtures.mjs: now is Nov 14 2026, 10:12 UTC. Not a test itself,
// and never installed.

import { at, NOW, time } from './snapshots-fixtures.mjs';

export { at, NOW, time };

/** An account as `user list --json` gives it. */
export const account = (name, more = {}) => ({
  name,
  uid: 1000,
  full_name: '',
  home: `/users/${name}`,
  shell: '/system/commands/nukesh',
  admin: false,
  console: false,
  groups: [name],
  disabled: false,
  password: 'set',
  ...more,
});

/** The accounts of a small server: two administrators, a person with the console, a key-only account and one that is disabled. */
export const people = () => [
  account('chase', { uid: 1000, full_name: 'Chase', admin: true, console: true, groups: ['admins', 'chase', 'console'] }),
  account('alice', { uid: 1001, full_name: 'Alice Liddell', admin: true, console: true, groups: ['admins', 'alice', 'console'] }),
  account('sam', { uid: 1002, full_name: 'Sam', console: true, groups: ['sam', 'console', 'photos'] }),
  account('backup', { uid: 1003, shell: '', groups: ['backup'], password: 'none' }),
  account('old', { uid: 1004, groups: ['old'], disabled: true }),
];

/** The same accounts as a person who is not root lists them: whether each is disabled and has a password is root's to read. */
export const peopleAsAPerson = () => people().map((one) => ({ ...one, disabled: null, password: null }));

/** What `lastlog --json` gives for those accounts. */
export const lastlog = () => [
  { name: 'chase', sign_in: { at_ms: at(2026, 11, 14, 7, 12), terminal: 'web', from: '192.168.1.25', via: 'console' }, failed: 0, last_failed: null },
  { name: 'alice', sign_in: { at_ms: at(2026, 11, 13, 19, 40), terminal: 'pts/0', from: '10.8.0.2', via: 'ssh' }, failed: 0, last_failed: null },
  { name: 'sam', sign_in: { at_ms: at(2026, 11, 8, 9, 5), terminal: 'smb', from: '192.168.1.30' }, failed: 0, last_failed: null },
  { name: 'backup', sign_in: null, failed: 0, last_failed: null },
  { name: 'old', sign_in: null, failed: 0, last_failed: null },
];

/** The rules of `admin settings --json`'s `sign_in`, as `GET /api/1/security/sign-in` answers. */
export const rules = () => ({ sign_in: { second_step: 'administrators', session_hours: 12 } });

/** A key line's fingerprint, whole, made from `seed`. */
export const fingerprint = (seed) => `SHA256:${seed}${'x'.repeat(43 - seed.length)}`;

/** A key as `user show --json` gives it. */
export const key = (comment, seed, more = {}) => ({
  type: 'ssh-ed25519', comment, fingerprint: fingerprint(seed), line: 3, options: false, ...more,
});

/** A key line as ssh-keygen writes it: the type, a base64 blob that begins with the type's own words, and a comment. */
export function keyLine(type = 'ssh-ed25519', comment = 'laptop') {
  const name = Buffer.from(type);
  const length = Buffer.alloc(4);
  length.writeUInt32BE(name.length);
  return `${type} ${Buffer.concat([length, name, Buffer.alloc(32, 7)]).toString('base64')} ${comment}`;
}

/** `user show --json`: the account, its keys, second steps, tokens and sessions at the machine. */
export const shown = (name, more = {}) => {
  const found = people().find((one) => one.name === name);
  return {
    account: { ...found, since_ms: at(2025, 10, 19, 8), password_changed_ms: at(2025, 10, 19, 8, 5) },
    keys: [key('laptop', 'pR8x'), key('phone', 'Lm3d')],
    steps: { app: true, passkeys: [{ name: 'Security key', id: 'abc' }], recovery_left: 8, trusted_browsers: 2 },
    tokens: [
      { name: 'ansible', scopes: ['*'], until_ms: at(2026, 12, 13) },
      { name: 'nas-snapshots', scopes: ['snapshots:read'], until_ms: at(2027, 2, 1) },
    ],
    sessions: [{
      id: '4123-1763110000', kind: 'ssh', terminal: 'pts/3', from: '10.8.0.2', since_ms: at(2026, 11, 13, 21, 40), processes: 5,
    }],
    ...more,
  };
};

/** The sessions route's answer (13.3): two console sessions, one of them the caller's, and a terminal. */
export const sessionsAnswer = (more = {}) => ({
  sessions: [
    {
      id: 's-2', browser: 'Firefox on Android', from: '192.168.1.40', since_ms: at(2026, 11, 14, 8, 30), last_ms: at(2026, 11, 14, 9, 50), this: false,
    },
    {
      id: 's-1', browser: 'Firefox on Linux', from: '192.168.1.25', since_ms: at(2026, 11, 14, 7, 12), last_ms: at(2026, 11, 14, 10, 11), this: true,
    },
  ],
  terminals: [{
    id: 3, title: 'build', panes: 2, admin: false, since_ms: at(2026, 11, 14, 9, 0), attached: true,
  }],
  ...more,
});
