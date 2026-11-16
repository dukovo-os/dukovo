// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the SSH card says (design 8.6 and 9.1) as plain values: the row, what to
// check while a try runs, and the SSH sign-ins since it began. They are made
// from `ssh-server show --json` (design 11.8) and `lastlog --json` (11.7).
// Nothing here touches the page.

import { counted } from '../../lib/size-words.js';

/** `names` as a list in a sentence: "alice", "alice and bob", "alice, bob and carol". */
export function listWords(names) {
  if (names.length < 2) return names.join('');
  return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`;
}

const names = (list) => (Array.isArray(list) ? list : []);

/** Whether the file does not read, so that the form cannot change it. */
export const hasMistakes = (show) => names(show?.mistakes).length > 0;

/** The first mistake of the file in words: "sshd_config has a mistake on line 7: …". */
export function mistakeWords(show) {
  const [first] = names(show?.mistakes);
  if (!first) return '';
  const why = [first.why, first.text && `(${String(first.text).trim()})`].filter(Boolean).join(' ');
  return `sshd_config has a mistake on line ${first.line}${why ? `: ${why}` : ''}.`;
}

/** How people sign in, as the row says it. */
function methodsWords(show) {
  if (show.passwords === null || show.keys === null || show.passwords === undefined || show.keys === undefined) return null;
  if (show.passwords && show.keys) return 'keys and passwords';
  if (show.keys) return 'keys only';
  return show.passwords ? 'passwords only' : 'no way to sign in';
}

/** Who may sign in, as the row says it, or null for everyone. */
export function whoWords(users, groups) {
  const parts = [];
  if (names(users).length > 0) parts.push(`only ${listWords(users)}`);
  if (names(groups).length > 0) parts.push(`only the ${groups.length === 1 ? 'group' : 'groups'} ${listWords(groups)}`);
  return parts.length === 0 ? null : parts.join(', and ');
}

/** The row, as the board has it: "Port 22 · keys and passwords · root cannot sign in", or "Off". */
export function rowWords(show) {
  if (show?.on === false) return 'Off';
  if (hasMistakes(show)) return `On${show.port ? ` · port ${show.port}` : ''} · the settings file has a mistake`;
  const parts = [
    show?.port ? `Port ${show.port}` : null,
    methodsWords(show ?? {}),
    whoWords(show?.allow_users, show?.allow_groups),
    'root cannot sign in',
    show?.on === true && show.running === false ? 'not running now' : null,
  ];
  return parts.filter(Boolean).join(' · ');
}

// ---- while a try runs --------------------------------------------------------------------------

/** The machine as `ssh` is told it: a name gets `.local`, an address or a full name is as it is. */
export const machineOf = (host) => (/[.:]/.test(host) ? host : `${host}.local`);

/** What to do while SSH's settings are tried (design 8.6): open a new connection, since only a new one tells. */
export const checkWords = 'Connections that were open stay open, so only a new one tells.';

/** The command to type: `ssh NAME@MACHINE.local`. */
export const connectCommand = (name, host) => `ssh ${name}@${machineOf(host)}`;

/** When the try that is running began, in Unix milliseconds, from the journal's rows (newest first); null when no row of ours says so. */
export function beganAt(rows) {
  for (const row of Array.isArray(rows) ? rows : []) {
    if (row.status === 0 && (row.action === 'ssh.set' || row.action === 'ssh.off') && Array.isArray(row.argv) && row.argv.includes('--try')) {
      return row.at * 1000;
    }
  }
  return null;
}

/**
 * The SSH sign-ins since `since` (Unix milliseconds) among the answer of
 * `accounts.last`, oldest first: `{name, at, from}`. A sign-in is over SSH
 * where its record says so (`via`), as a console terminal is on a
 * pseudo-terminal from an address too. Only an account's latest sign-in is
 * kept, so one that signed in twice is listed once.
 */
export function signInsSince(last, since) {
  return (Array.isArray(last) ? last : [])
    .filter((one) => one?.sign_in?.via === 'ssh' && one.sign_in.at_ms >= since)
    .map((one) => ({ name: one.name, at: one.sign_in.at_ms, from: one.sign_in.from ?? null }))
    .sort((a, b) => a.at - b.at);
}

/** One of those, as a line: "owner signed in over SSH at 2:28 PM from 192.168.1.25"; `time` is lib/time.js's. */
export const signInWords = (one, time) => `${one.name} signed in over SSH at ${time.clock(Math.floor(one.at / 1000))}${one.from ? ` from ${one.from}` : ''}`;

/** What stands under Keep while nobody has signed in, or how many have. */
export const keepNote = (rows) => (rows.length === 0 ? 'Nobody has signed in over SSH since the change.' : `${counted(rows.length, 'account')} signed in over SSH since the change.`);
