// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Databases pages say, as plain values. `databases.list` (`database
// list --json`) is read by `readOf` into one shape that the pages use, and each
// of its members may be missing or older than this file: a page shows what it
// has. The words of the list, of a database's page, of the dialogs and of the
// choices of Add a database are here, so that a test reaches every branch; the
// files beside this one only draw them.

import { bytesWords, counted } from '../../lib/size-words.js';
import { hrefOf } from '../../lib/database-route.js';

/** PostgreSQL's service, which the list's line links to and Turn it on starts. */
export const SERVICE = 'postgresql';

/** Where PostgreSQL is added from, when it is not installed. */
export const SHEET_PATH = '/apps/add/org.postgresql.postgresql';

/** The certificate PostgreSQL gets unless another is chosen. */
export const THIS_MACHINE = 'this-machine';

/** What a reader, and an administrator whose console is locked, are told in place of the list. */
export const READER_WORDS = 'Only an administrator can list the databases: listing them signs in to PostgreSQL as its superuser.';
export const LOCKED_WORDS = 'Listing the databases signs in to PostgreSQL as its superuser, so the console has to be unlocked first.';

/** What other machines call this one when the console cannot read its name: the name it was reached by. */
export const guessMachine = (host) => ({ name: host, first: host.split('.')[0], host });

const isObject = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
const text = (value) => (typeof value === 'string' ? value : '');
const number = (value) => (Number.isInteger(value) && value >= 0 ? value : null);
const objects = (value) => (Array.isArray(value) ? value.filter(isObject) : []);
const names = (value) => (Array.isArray(value) ? value.map(String) : text(value) === '' ? [] : text(value).split(',').map((one) => one.trim()));

/** The number of PostgreSQL's version, from `version()`'s sentence or the number alone. */
const versionOf = (value) => /(\d+(?:\.\d+)*)/.exec(text(value))?.[1] ?? '';

/** The name of the certificate in `value`, which is its name or the path of the copy PostgreSQL reads. */
const certificateOf = (value) => /(?:^|\/)postgresql\/([^/]+)\/fullchain\.pem$/.exec(text(value))?.[1] ?? (text(value) !== '' && !text(value).includes('/') ? text(value) : null);

/**
 * `databases.list`'s answer as one value: `{server, databases, roles,
 * access, problems}`, with each list sorted as it is shown. Null when the
 * answer is not one.
 */
export function readOf(answer) {
  if (!isObject(answer)) return null;
  const server = isObject(answer.server) ? answer.server : {};
  return {
    server: {
      installed: server.installed === true,
      enabled: server.enabled === true,
      running: server.running === true,
      version: versionOf(server.version),
      listen: text(server.listen),
      ssl: server.ssl === true || server.ssl === 'on',
      certificate: certificateOf(server.certificate),
    },
    databases: objects(answer.databases).filter((one) => text(one.name) !== '').map((one) => ({
      name: one.name,
      owner: text(one.owner),
      encoding: text(one.encoding),
      provider: text(one.provider),
      locale: text(one.locale),
      bytes: number(one.bytes),
      sessions: number(one.sessions) ?? 0,
      site: text(one.site) || null,
      madeHere: one.made_by_database === true,
      mismatch: one.collation_mismatch === true,
    })).sort((one, other) => one.name.localeCompare(other.name)),
    roles: objects(answer.roles).filter((one) => text(one.name) !== '').map((one) => ({
      name: one.name, login: one.login !== false, password: one.password === true, madeHere: one.made_by_database === true,
    })),
    access: objects(answer.access).map((one) => ({
      line: number(one.line),
      type: text(one.type),
      databases: names(one.databases),
      users: names(one.users),
      address: text(one.address),
      method: text(one.method),
      byHand: one.by_hand === true,
    })),
    problems: objects(answer.problems).map((one) => ({ id: text(one.id) || null, words: text(one.words), line: number(one.line) })),
  };
}

// ---- the server ---------------------------------------------------------------

/** What the server is: `missing`, `off`, `stopped` (enabled, and not running) or `on`. */
export function serverState(server) {
  if (!server.installed) return 'missing';
  if (server.running) return 'on';
  return server.enabled ? 'stopped' : 'off';
}

/** What the list says when PostgreSQL does not run. */
export const STATE_WORDS = Object.freeze({
  missing: 'PostgreSQL is not installed.',
  off: 'PostgreSQL is off.',
  stopped: 'PostgreSQL is not running.',
});

/** The line over the list: the version, that it runs, and where it listens. */
export function serverLine(server) {
  const away = server.listen !== '' && !['localhost', '127.0.0.1', '::1'].includes(server.listen);
  const where = away
    ? `listens on the network, TLS with ${server.certificate ?? THIS_MACHINE}'s certificate`
    : 'listens on this machine only';
  return [`PostgreSQL${server.version ? ` ${server.version}` : ''}`, 'running', where].join(' · ');
}

// ---- who connects -------------------------------------------------------------

/**
 * Who connects to `db`: `{role, password, peer, networks, hand}`. The role
 * is the owner's; `password` says that it signs in with one, which the
 * bundle's lines for this server let in; `peer` is its name when it signs in
 * as the account of that name; `networks` are the lines `database` wrote for
 * it, `{address, line}`; `hand` the lines written by hand that name it.
 */
export function accessOf(read, db) {
  const role = read.roles.find((one) => one.name === db.owner) ?? null;
  const lines = read.access.filter((one) => one.databases.includes(db.name));
  return {
    role,
    password: role?.password === true,
    peer: role !== null && role.login && !role.password ? role.name : null,
    networks: lines.filter((one) => !one.byHand && one.type === 'hostssl').map((one) => ({ address: one.address, line: one.line })),
    hand: lines.filter((one) => one.byHand),
  };
}

/** The ways `access` says a program connects: `local`, `socket` and `network`, in that order. */
export function waysOf(access) {
  return [access.password && 'local', access.peer !== null && 'socket', access.networks.length > 0 && 'network'].filter(Boolean);
}

/** The words of a line of `pg_hba.conf` that was written by hand. */
export const handWords = (line) => [line.type, line.databases.join(','), line.users.join(','), line.address, line.method].filter(Boolean).join(' ');

/** The lines of the Who connects cell of a row, one each. */
export function whoLines(access) {
  const lines = [];
  if (access.password) lines.push('Programs on this server, with the password');
  if (access.peer !== null) lines.push(`${access.peer}, by its account`);
  for (const network of access.networks) lines.push(`${network.address}, over TLS with the password`);
  if (access.hand.length > 0) lines.push(`and ${counted(access.hand.length, 'line')} written by hand`);
  return lines.length === 0 ? ['No one is known to connect'] : lines;
}

/**
 * Whether taking `from` (one network, or every one when null) away from the
 * database `name` leaves no line that lets other machines in, which sends
 * PostgreSQL back to listening on this machine only, and restarts it.
 */
export function denyRestarts(read, name, from = null) {
  const mine = (line) => line.databases.includes(name) && (from === null || line.address === from);
  return !read.access.some((line) => !line.byHand && line.type === 'hostssl' && !mine(line));
}

// ---- a database ---------------------------------------------------------------

/** The text and sorting of `db`, as Add a database words them. */
export function localeWords(db) {
  if (db.provider === 'builtin' && db.locale === 'C.UTF-8') return 'UTF-8, sorted by character code, the same on every machine';
  if (db.provider === 'libc' && db.locale === 'en_US.UTF-8') return 'UTF-8, sorted as English (US)';
  const encoding = db.encoding === 'UTF8' ? 'UTF-8' : db.encoding;
  return [encoding, db.locale && `sorted as ${db.locale}`].filter(Boolean).join(', ') || 'Not known';
}

/** The rows of the table of databases. */
export function rowsOf(read) {
  return read.databases.map((db) => ({
    name: db.name,
    href: hrefOf(db.name),
    owner: db.owner,
    site: db.site,
    mismatch: db.mismatch,
    who: whoLines(accessOf(read, db)),
    size: db.bytes === null ? 'Not known' : bytesWords(db.bytes),
    sessions: db.sessions,
  }));
}

/** The facts of a database's page, `{label, value}`; `site` is a link and is drawn by the page. */
export function factsOf(db) {
  return [
    { label: 'Owner', value: db.owner || 'Not known' },
    { label: 'Text and sorting', value: localeWords(db) },
    { label: 'Size', value: db.bytes === null ? 'Not known' : bytesWords(db.bytes) },
    { label: 'Sessions', value: String(db.sessions) },
  ];
}

/** The words over the sessions of a database. */
export const sessionsWords = (count) => (count === 0 ? 'No one is connected now.' : `${counted(count, 'program')} ${count === 1 ? 'is' : 'are'} connected now.`);

/** The two commands that bring a database's indexes up to date after the system's sorting data changed. */
export const reindexLines = (name) => [
  `elevate psql -U postgres -d ${name} -c 'REINDEX DATABASE ${name}'`,
  `elevate psql -U postgres -d ${name} -c 'ALTER DATABASE ${name} REFRESH COLLATION VERSION'`,
];

/** The title of the words that say a database's indexes are out of date. */
export const MISMATCH_WORDS = 'Its sorting data changed in an update: rebuild its indexes, then tell PostgreSQL that it did.';

/** What PostgreSQL's own check said of `pg_hba.conf`: `{words, line}`. */
export const problemWords = (problem) => `${problem.words}${problem.line === null ? '' : ` (line ${problem.line} of pg_hba.conf)`}`;

// ---- Remove… ------------------------------------------------------------------

/**
 * What Remove… offers for `db`: whether the role can go with it (it was made
 * by `database add`), whether that is ticked (it owns no other database in
 * the list), what is said of the sessions, and the words that the role stays.
 */
export function removeOf(read, db) {
  const role = read.roles.find((one) => one.name === db.owner) ?? null;
  const others = read.databases.filter((one) => one.owner === db.owner && one.name !== db.name).length;
  const offered = role !== null && role.madeHere;
  return {
    offered,
    ticked: offered && others === 0,
    stays: role === null ? null : offered ? null : `The role ${role.name} was not made by this page, so it stays.`,
    shared: offered && others > 0 ? `The role ${role.name} also owns ${counted(others, 'other database')}.` : null,
    sessions: db.sessions,
  };
}

/** What the Remove dialog says the removal does. */
export const removeLead = (name) => `Removes the database ${name} and everything in it. Its data stays in the snapshots of /shared until they are pruned.`;

/** What it says of the open sessions, which are ended by it. */
export const endsSessions = (count) => `Ends its ${counted(count, 'open session')}.`;

// ---- the backups --------------------------------------------------------------

/** How often a place makes its backups, as a word before "backup". */
const OFTEN = Object.freeze({ day: 'nightly', week: 'weekly', hand: 'by-hand' });

/**
 * What Add a database says of the backups, from `backup list --json`:
 * `{words, covered}`; `covered` is false when no place copies `/shared`, and
 * null when the places could not be read.
 */
export function backupWords(backups) {
  const places = Array.isArray(backups?.places) ? backups.places : [];
  const place = places.find((one) => Array.isArray(one?.places) && one.places.includes('/shared'));
  const hook = 'Before each snapshot of /shared, PostgreSQL\'s backup hook asks it for a checkpoint, so the copy starts as after a power cut, which PostgreSQL recovers from.';
  if (backups == null) return { covered: null, words: hook };
  if (!place) return { covered: false, words: `${hook} No backup place copies /shared yet.` };
  return { covered: true, words: `${hook} /shared is in the ${OFTEN[place.every] ?? 'regular'} backup to ${place.name}.` };
}

// ---- the choices of Add a database --------------------------------------------

/** Whether `name`, one of a certificate's names, names `host`: itself, or for `*.example.org` one word under it. */
const names1 = (name, host) => name === host || (name.startsWith('*.') && host.endsWith(name.slice(1)) && !host.slice(0, host.length - name.length + 1).includes('.'));

/** Whether `names` (a certificate's) name `host`. */
export const covers = (names, host) => (Array.isArray(names) ? names : []).some((one) => typeof one === 'string' && names1(one, host));

/**
 * The certificates that can be chosen, from `cert list --json`: this
 * machine's first, then each that has not ended or been revoked, `{value,
 * label, names}`.
 */
export function certificateChoices(answer) {
  const list = Array.isArray(answer) ? answer : Array.isArray(answer?.certificates) ? answer.certificates : [];
  const usable = list.filter((one) => text(one?.name) !== '' && one.name !== THIS_MACHINE && one.revoked !== true && !(Number.isFinite(one.days_left) && one.days_left < 0));
  const own = list.find((one) => one?.name === THIS_MACHINE);
  return [
    { value: THIS_MACHINE, label: 'This machine\'s own', names: Array.isArray(own?.names) ? own.names : null },
    ...usable.map((one) => ({ value: one.name, label: one.name, names: Array.isArray(one.names) ? one.names : null })),
  ];
}

/** The accounts of `accounts.list` that a database can be tied to: not disabled, and with a name PostgreSQL's rule allows for a role. */
export function accountChoices(answer) {
  const list = Array.isArray(answer) ? answer : Array.isArray(answer?.accounts) ? answer.accounts : [];
  const named = list.filter((one) => text(one?.name) !== '' && one.disabled !== true);
  const fit = named.filter((one) => /^[a-z][a-z0-9_]{0,62}$/.test(one.name) && !one.name.startsWith('pg_') && !['postgres', 'template0', 'template1'].includes(one.name));
  return { names: fit.map((one) => one.name).sort(), left: named.length - fit.length };
}

/** The line that tells which accounts are not offered, or null. */
export const leftOutWords = (count) => (count === 0 ? null : `${counted(count, 'account')} ${count === 1 ? 'is' : 'are'} not offered: the name of a role is made of lowercase letters, digits and _.`);
