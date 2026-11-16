// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Accounts pages say of an account (design 3.5, 7.4, 7.5 and 7.7), as
// plain values: what each account can do, when it was last active, its keys,
// second steps and tokens, and what it is signed in with. They are made from
// the JSON of `user list`, `user show` and `lastlog` (design 11.7) and of the
// sessions route (13.3); times are Unix milliseconds. Nothing here touches
// the page.

import { counted } from './size-words.js';
import { dayWords } from './snapshot-words.js';
import { makeTime } from './time.js';

/** How many recovery codes a second step makes at a time (C2a), for "8 of 10 left". */
export const RECOVERY_CODES = 10;

// ---- the account -------------------------------------------------------------

/**
 * Whether `account` is disabled. `user list` leaves `disabled` null for
 * anyone but root, who gets false or a value that says since when.
 */
export const isDisabled = (account) => account.disabled != null && account.disabled !== false;

/** The letter that stands for `account` in its circle. */
export const initialOf = (account) => account.name.charAt(0).toUpperCase();

/** How many colours the circles have. */
export const SHADES = 4;

/** Which of the `SHADES` colours `account`'s circle has, the same each time for the same name. */
export function shadeOf(account) {
  let sum = 0;
  for (let at = 0; at < account.name.length; at += 1) sum += account.name.charCodeAt(at);
  return sum % SHADES;
}

/**
 * What `account` can do, in a few words for the list's Can column. `keys`
 * is how many SSH keys it has, from `user show`, or null when that is not
 * known; it matters only to an account with no password, which can sign in
 * with a key alone or not at all. An administrator can do all there is, so
 * it is told by that one word.
 */
export function canWords(account, keys = null) {
  if (isDisabled(account)) return 'Disabled';
  if (account.password === 'none') {
    if (keys === null) return 'No password';
    return keys > 0 ? 'SSH key only' : 'Cannot sign in';
  }
  if (account.admin) return 'Administrator';
  const shell = typeof account.shell === 'string' && account.shell !== '';
  if (account.console && shell) return 'Console and shell';
  if (account.console) return 'Console';
  return shell ? 'Shell' : 'Files only';
}

/** The headline of one account: "Administrator · account 1001 · since Oct 19"; `since_ms` is left out when the machine does not keep it. */
export function accountLine(account, zone, now = Date.now()) {
  const since = Number.isFinite(account.since_ms) ? `since ${dayWords(account.since_ms, zone, now)}` : null;
  const number = `${account.admin ? 'account' : 'Account'} ${account.uid}`;
  return [account.admin ? 'Administrator' : null, number, since].filter(Boolean).join(' · ');
}

/**
 * The password row in words: "Set, changed Oct 19", "Set", "None", or null
 * when the answer does not say (a person who is not root is not told
 * of another's).
 */
export function passwordWords(account, zone, now = Date.now()) {
  if (account.password === 'none') return 'None';
  if (account.password !== 'set') return null;
  if (!Number.isFinite(account.password_changed_ms)) return 'Set';
  return `Set, changed ${dayWords(account.password_changed_ms, zone, now)}`;
}

// ---- the second step, keys and tokens -------------------------------------------

/**
 * The second step of `steps` (`user show`'s) as `{summary, extras}`:
 * "Authenticator app and 1 passkey" with "8 of 10 recovery codes left" and
 * "2 trusted browsers" beside it, or "None", which has no extras: recovery
 * codes and trusted browsers belong to a second step that is there.
 */
export function stepsWords(steps) {
  const passkeys = Array.isArray(steps?.passkeys) ? steps.passkeys.length : 0;
  const kinds = [steps?.app ? 'Authenticator app' : null, passkeys > 0 ? counted(passkeys, 'passkey') : null].filter(Boolean);
  if (kinds.length === 0) return { summary: 'None', extras: [] };
  const extras = [];
  if (steps.recovery_left > 0) extras.push(`${steps.recovery_left} of ${RECOVERY_CODES} recovery codes left`);
  if (steps.trusted_browsers > 0) extras.push(counted(steps.trusted_browsers, 'trusted browser'));
  return { summary: kinds.join(' and '), extras };
}

/** The key types authd takes, as a person reads them. */
const KEY_TYPES = Object.freeze({
  'ssh-ed25519': 'ED25519',
  'ecdsa-sha2-nistp256': 'ECDSA P-256',
  'ecdsa-sha2-nistp384': 'ECDSA P-384',
});

/** A fingerprint as the list shows it, "SHA256:pR8x…1kQ": its start and end, which tell two keys apart. */
export const shortFingerprint = (fingerprint) => (fingerprint.length > 16 ? `${fingerprint.slice(0, 11)}…${fingerprint.slice(-3)}` : fingerprint);

/**
 * One of `user show`'s keys, `{type, comment, fingerprint, line, options}`,
 * as `{name, type, short, full, options}`: the comment, or words that say
 * there is none, the type, the fingerprint short and whole, and whether the
 * line has options, which authd passes over.
 */
export function keyWords(key) {
  return {
    name: key.comment || 'A key with no name',
    type: KEY_TYPES[key.type] ?? key.type,
    short: shortFingerprint(key.fingerprint),
    full: key.fingerprint,
    options: key.options === true,
  };
}

/** A scope's area in words: "snapshots" is "Snapshots", "ssh-keys" is "Ssh keys". */
const areaWords = (area) => {
  const words = area.replaceAll('-', ' ');
  return words.charAt(0).toUpperCase() + words.slice(1);
};

/**
 * What a token may do, from its `scopes` (C2 section 9: `*`, an area, or an
 * area with `:read`), for the account `owner`: "Everything chase can do",
 * "Snapshots: read".
 */
export function scopesWords(scopes, owner) {
  if (scopes.includes('*')) return `Everything ${owner} can do`;
  return scopes.map((scope) => {
    const [area, access] = scope.split(':');
    return `${areaWords(area)}: ${access === 'read' ? 'read' : 'read and change'}`;
  }).join(', ');
}

/** One of `user show`'s tokens, `{name, scopes, until_ms}`, as `{name, detail}`: what it may do and when it runs out. */
export function tokenWords(token, owner, zone, now = Date.now()) {
  const ends = Number.isFinite(token.until_ms)
    ? `${token.until_ms > now ? 'expires' : 'expired'} ${dayWords(token.until_ms, zone, now)}`
    : null;
  return { name: token.name, detail: [scopesWords(token.scopes, owner), ends].filter(Boolean).join(' · ') };
}

// ---- when ---------------------------------------------------------------------

/** "Today", "Yesterday", "Nov 8" or "Nov 8, 2025": the day of `ms` on the clocks of `zone`. */
export function dayLabel(ms, zone, now = Date.now()) {
  const words = makeTime(zone, () => now).when(ms / 1000);
  if (/^\d/.test(words)) return 'Today';
  return words.startsWith('Yesterday ') ? 'Yesterday' : dayWords(ms, zone, now);
}

/** "since 7:12 AM", "since yesterday 7:40 PM", "since Nov 8 9:05 PM": the time of `ms` after "since". */
export function sinceWords(ms, zone, now = Date.now()) {
  const words = makeTime(zone, () => now).when(ms / 1000);
  return `since ${words.startsWith('Yesterday ') ? `yesterday ${words.slice('Yesterday '.length)}` : words}`;
}

/** The terminals that name a place of their own in `lastlog`'s records. */
const TERMINAL_WORDS = Object.freeze({ web: 'web console', smb: 'file sharing', nfs: 'file sharing' });

/**
 * The place of a sign-in of `lastlog --json` (`{terminal, via}`) in words:
 * "SSH", "web console", "console terminal", "file sharing", "at the
 * machine", or a terminal's own name where the record says no more, as an
 * older record of a pseudo-terminal, which may be SSH or the console's.
 */
export function signInWords({ terminal = '', via = null } = {}) {
  if (via === 'ssh') return 'SSH';
  if (via === 'console') return terminal === 'web' ? 'web console' : 'console terminal';
  if (TERMINAL_WORDS[terminal]) return TERMINAL_WORDS[terminal];
  if (/^tty\d+$/.test(terminal)) return 'at the machine';
  return terminal;
}

/**
 * An account's Last active, from its entry of `lastlog --json`
 * (`{sign_in: {at_ms, terminal, from, via} | null}`): "Yesterday, SSH", "Nov 8,
 * file sharing", "Never". `own` is the account of the person looking, whose
 * session this browser is.
 */
export function lastWords(last, zone, { now = Date.now(), own = false } = {}) {
  if (own) return 'Now, this browser';
  const signIn = last?.sign_in;
  if (!signIn || !Number.isFinite(signIn.at_ms)) return 'Never';
  return `${dayLabel(signIn.at_ms, zone, now)}, ${signInWords(signIn)}`;
}

// ---- what an account is signed in with -------------------------------------------

/** How a session at the machine is said, by the `kind` that `user show` gives: its title, and what it is called in a sentence. */
const MACHINE_WORDS = Object.freeze({
  ssh: ['SSH', 'the SSH session'],
  screen: ['The machine\'s screen', 'the session on the machine\'s screen'],
  serial: ['A serial line', 'the session on a serial line'],
  other: ['Another session', 'the session'],
});

/**
 * One thing an account is signed in with, as a row of "Signed in now":
 * `{title, detail, since, what}`, where `what` names it in a button or a
 * question ("the terminal build"). `entry.kind` is `console` for an entry of
 * the sessions route's `sessions` (13.3), `terminal` for one of its
 * `terminals`, and for a session at the machine the `kind` of `user show`.
 */
export function sessionWords(entry, zone, now = Date.now()) {
  const since = Number.isFinite(entry.since_ms) ? sinceWords(entry.since_ms, zone, now) : '';
  const from = entry.from ? ` from ${entry.from}` : '';
  if (entry.kind === 'console') {
    const last = Number.isFinite(entry.last_ms) ? `last used ${makeTime(zone, () => now).when(entry.last_ms / 1000)}` : null;
    return {
      title: entry.this ? 'This browser' : entry.browser || 'A browser',
      detail: (entry.this ? [entry.browser, entry.from] : [entry.from, last]).filter(Boolean).join(' · '),
      since,
      what: `${entry.browser ? `the session in ${entry.browser}` : 'the session in a browser'}${from}`,
    };
  }
  if (entry.kind === 'terminal') {
    const name = entry.title || String(entry.id);
    return {
      title: `Terminal ${name}`,
      detail: [counted(entry.panes, 'pane'), entry.admin ? 'with an administrator shell' : null, entry.attached ? null : 'not open in a browser']
        .filter(Boolean).join(' · '),
      since,
      what: `the terminal ${name}`,
    };
  }
  const [title, what] = MACHINE_WORDS[entry.kind] ?? MACHINE_WORDS.other;
  return {
    title,
    detail: [entry.from, entry.terminal, entry.processes === 1 ? '1 process' : `${entry.processes} processes`].filter(Boolean).join(' · '),
    since,
    what: `${what}${from}`,
  };
}

/**
 * Everything an account is signed in with, as rows in the order the page
 * lists them: console sessions with this browser first, then terminals,
 * then sessions at the machine. `lists` is `{sessions, terminals, machine}`:
 * the sessions route's two lists (13.3) and `user show`'s `sessions`. A row
 * is `{scope, id, this, title, detail, since, what}`; `scope` is `console`,
 * `terminal` or `machine`, which End asks different routes for. A console
 * terminal's shells are sessions at the machine too, which `user show` cannot
 * tell from SSH: those on a pseudo-terminal that a terminal names in `ttys`
 * are its row's, and not listed again.
 */
export function signedIn({ sessions = [], terminals = [], machine = [] }, zone, now = Date.now()) {
  const bySince = (a, b) => (a.since_ms ?? 0) - (b.since_ms ?? 0);
  const rowOf = (scope, entry, kind = scope) => ({
    scope, id: entry.id, this: entry.this === true, ...sessionWords({ ...entry, kind }, zone, now),
  });
  const consoles = [...sessions].sort((a, b) => Number(b.this === true) - Number(a.this === true) || bySince(a, b));
  const terminalsOwn = new Set(terminals.flatMap((entry) => entry.ttys ?? []));
  return [
    ...consoles.map((entry) => rowOf('console', entry)),
    ...[...terminals].sort(bySince).map((entry) => rowOf('terminal', entry)),
    ...machine.filter((entry) => !terminalsOwn.has(entry.terminal)).sort(bySince).map((entry) => rowOf('machine', entry, entry.kind)),
  ];
}
