// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Databases pages know of a database, as plain values (the pages
// that draw them are pages/databases/): the rule for a name and the name a
// site's database gets, the three ways to connect and the settings and
// examples of each, the networks that other machines connect from, the
// arguments of each action, and the answer that holds a password. The
// examples and the settings never hold the password: nothing here is given
// one, except `settingsFile`, which makes the text that is shown once.

import {
  formatNetwork, networkOf, parseNetwork,
} from './address-model.js';

// The addresses of the pages are in their own file, which the list reads
// before anything else here is needed.
export { hrefOf, routeOf } from './database-route.js';

/** PostgreSQL's port. */
export const PORT = 5432;

/** The address programs on this server connect to. */
export const LOCAL_HOST = '127.0.0.1';

/** The folder of PostgreSQL's socket, which an account of this server connects through. */
export const SOCKET_DIR = '/system/live/services/postgres';

/** The most networks one database may be let in from. */
export const NETWORKS_MAX = 20;

/** The word that is typed to let every address in. */
export const ANYWHERE = 'anywhere';

/** The longest name a database or role has. */
export const NAME_MAX = 63;

/** The names PostgreSQL keeps for itself. */
const RESERVED = Object.freeze(['postgres', 'template0', 'template1']);

/** A name of a database or role: it starts with a letter, and has lowercase letters, digits and `_`. */
const NAME_PATTERN = /^[a-z][a-z0-9_]{0,62}$/;

/** A DNS name, as a website's is: lowercase words of letters, digits and `-`, 253 bytes at most. */
const SITE_PATTERN = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?(\.[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)*$/;

const isObject = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);

/** Whether `name` is one PostgreSQL keeps for itself: `pg_` and what follows, `postgres` and the two templates. */
export const isReserved = (name) => name.startsWith('pg_') || RESERVED.includes(name);

/**
 * Why `name` cannot name a database or a role (`kind` is the word that
 * goes in the sentence), in words; empty text when it can.
 */
export function nameProblem(name, kind = 'database') {
  if (name === '') return `Give the ${kind} a name.`;
  if (!NAME_PATTERN.test(name)) return 'Use lowercase letters, digits and _, start with a letter, and keep it to 63 characters.';
  if (isReserved(name)) return 'PostgreSQL keeps the names that start with pg_, and postgres, template0 and template1.';
  return '';
}

/** Whether `text` is a DNS name a site may have, which `?site=` of the page may hold. */
export const isSiteName = (text) => typeof text === 'string' && text.length <= 253 && SITE_PATTERN.test(text);

/**
 * The name of the database for `site`: its first word, with each character
 * the rule does not allow made `_`, and a number after it (`shop_2`) when
 * that name is one of `taken`, which holds the databases and the roles there
 * are. A word that does not start with a letter, or starts with `pg_`, has
 * `db_` before it. Empty text when `site` is not a name.
 */
export function nameFromSite(site, taken = []) {
  if (!isSiteName(site)) return '';
  let base = site.split('.')[0].replace(/[^a-z0-9_]/g, '_');
  if (!/^[a-z]/.test(base) || base.startsWith('pg_')) base = `db_${base}`;
  const used = new Set(taken);
  const free = (name) => !used.has(name) && !isReserved(name);
  base = base.slice(0, NAME_MAX);
  if (free(base)) return base;
  for (let number = 2; ; number += 1) {
    const suffix = `_${number}`;
    const name = `${base.slice(0, NAME_MAX - suffix.length)}${suffix}`;
    if (free(name)) return name;
  }
}

// ---- who connects -----------------------------------------------------------

/** The three ways a program connects, as the Add page offers them. */
export const WAYS = Object.freeze([
  {
    id: 'local',
    title: 'Programs on this server, with a password',
    words: (role) => `They connect to ${LOCAL_HOST}, port ${PORT}, as ${role}, with a long password made now and shown once. PHP's pages connect this way.`,
  },
  {
    id: 'socket',
    title: 'An account of this server, by the socket',
    words: () => 'The role is named for the account and signs in as it, with no password, through PostgreSQL\'s socket.',
    note: 'A service of another bundle cannot reach the socket yet: for a website\'s PHP, choose a password.',
  },
  {
    id: 'network',
    title: 'Programs on other machines, with a password',
    words: (role) => `Lets the addresses you name sign in as ${role}, with the password, over TLS with this server's certificate. PostgreSQL restarts once to listen on the network: its open sessions end.`,
  },
]);

/** The text and sorting a database can be made with: PostgreSQL's `en_US.UTF-8` of the C library, or its own `C.UTF-8`. */
export const LOCALES = Object.freeze([
  {
    value: 'en_US.UTF-8',
    label: 'UTF-8, sorted as English (US)',
    note: 'If the system\'s language data changes in an update, PostgreSQL asks for its indexes to be rebuilt; the Databases page will say so.',
  },
  { value: 'C.UTF-8', label: 'UTF-8, sorted by character code, the same on every machine', note: null },
]);

/** The text and sorting a database gets unless another is chosen. */
export const DEFAULT_LOCALE = LOCALES[0].value;

/** The words of the way `id`, for the role `role`. */
export const wayWords = (id, role) => WAYS.find((way) => way.id === id).words(role);

// ---- the networks other machines connect from -------------------------------

/**
 * The networks in `text`, one a line: `{networks, problems, anywhere,
 * tooMany}`. `networks` are the lines as PostgreSQL's file writes them, with
 * none twice; `problems` are `{line, text, why}` for a line that is not an
 * address with its prefix length, or has bits set after its prefix;
 * `anywhere` says that a line lets every address in.
 */
export function parseNetworks(text) {
  const networks = [];
  const problems = [];
  let anywhere = false;
  String(text ?? '').split('\n').forEach((raw, at) => {
    const line = raw.trim();
    if (line === '') return;
    const found = line.includes('/') ? parseNetwork(line) : null;
    if (found === null) {
      problems.push({ line: at + 1, text: line, why: `${line} is not an address with its prefix length, such as 192.168.1.0/24.` });
      return;
    }
    const first = networkOf(found);
    const written = formatNetwork(first);
    if (formatNetwork(found) !== written) {
      problems.push({ line: at + 1, text: line, why: `${line} has bits set after its prefix. Write ${written}.` });
      return;
    }
    if (found.prefix === 0) anywhere = true;
    if (!networks.includes(written)) networks.push(written);
  });
  return {
    networks, problems, anywhere, tooMany: networks.length > NETWORKS_MAX,
  };
}

/** Why the networks `found` (`parseNetworks`'s) cannot be sent, in words; empty text when they can. */
export function networksProblem(found) {
  if (found.problems.length > 0) return found.problems[0].why;
  if (found.networks.length === 0) return 'Give at least one network, one a line, such as 192.168.1.0/24.';
  if (found.tooMany) return `Give at most ${NETWORKS_MAX} networks.`;
  return '';
}

/** Whether `found` lets every address in and `typed` is not yet the word that says the person means it. */
export const needsAnywhere = (found, typed) => found.anywhere && String(typed).trim() !== ANYWHERE;

// ---- the connection settings ------------------------------------------------

/**
 * The name this machine is reached by from others, from `machine.name`
 * (`{name, first, domain, local}`): its full name when it has a domain,
 * else the name the local network answers to. Null when the answer holds no
 * name.
 */
export function machineOf(answer) {
  const name = typeof answer?.name === 'string' && answer.name !== '' ? answer.name : null;
  if (name === null) return null;
  const local = typeof answer.local === 'string' && answer.local !== '' ? answer.local : null;
  const first = typeof answer.first === 'string' && answer.first !== '' ? answer.first : name.split('.')[0];
  return { name, first, host: typeof answer.domain === 'string' && answer.domain !== '' ? name : (local ?? name) };
}

/** The file the server's certificate is saved as, for `machine`. */
export const rootFile = (machine) => `${machine.first}.pem`;

/**
 * What a program needs to connect to `database` the way `way` says: `rows`
 * (`{label, value}`) and `examples` (`{label, text}`, each to copy). `role`
 * is the user; `machine` is `machineOf`'s, needed for `network`. The
 * password is never here: a row says where it is, and the examples read it
 * from where the person keeps it.
 */
export function connectionSettings({
  way, database, role, machine = null,
}) {
  if (way === 'socket') {
    return {
      rows: [
        { label: 'Host', value: SOCKET_DIR },
        { label: 'Port', value: String(PORT) },
        { label: 'Database', value: database },
        { label: 'User', value: role },
        { label: 'Password', value: 'None: PostgreSQL knows the account' },
        { label: 'TLS', value: 'Not needed' },
      ],
      examples: [
        { label: `psql, as ${role} on this server`, text: `psql -d ${database}` },
        { label: `psql, with the socket named, as ${role}`, text: `psql "host=${SOCKET_DIR} dbname=${database} user=${role}"` },
      ],
    };
  }
  const away = way === 'network';
  const host = away ? machine?.host ?? '' : LOCAL_HOST;
  const root = away && machine ? rootFile(machine) : '';
  const tls = away ? `;sslmode=verify-full;sslrootcert=${root}` : '';
  const words = away ? ` sslmode=verify-full sslrootcert=${root}` : '';
  const query = away ? `?sslmode=verify-full&sslrootcert=${root}` : '';
  return {
    rows: [
      { label: 'Host', value: host },
      { label: 'Port', value: String(PORT) },
      { label: 'Database', value: database },
      { label: 'User', value: role },
      { label: 'Password', value: 'The one shown once' },
      { label: 'TLS', value: away ? 'Required: sslmode=verify-full, with this server\'s certificate' : 'Not needed' },
    ],
    examples: [
      { label: 'PHP', text: `new PDO('pgsql:host=${host};port=${PORT};dbname=${database}${tls}', '${role}', $password);` },
      { label: 'psql', text: `psql "host=${host} dbname=${database} user=${role}${words}"` },
      { label: 'A URL', text: `postgresql://${role}@${host}:${PORT}/${database}${query}` },
    ],
  };
}

/**
 * The text that is shown once and saved as `shop-postgresql.txt`: the
 * connection settings with `password` in its place. The only function here
 * that is given a password; what it makes goes to lib/shown-once.js and
 * nowhere else.
 */
export function settingsFile({
  way, database, role, password, machine = null,
}) {
  const { rows } = connectionSettings({
    way, database, role, machine,
  });
  const shown = rows.map(({ label, value }) => `${label}: ${label === 'Password' && password ? password : value}`);
  return [
    `PostgreSQL database ${database}`,
    '',
    ...shown,
    ...(way === 'network' && machine ? [`Root certificate: ${rootFile(machine)}`] : []),
    '',
    'The password is shown only once. Keep this file where only you can read it.',
    '',
  ].join('\n');
}

/** The name of the file that Save as file gives for `database`. */
export const fileName = (database) => `${database}-postgresql.txt`;

// ---- the answers that hold a password ---------------------------------------

/**
 * What `database add --json` and `database password --json` answered, from
 * the `output` lines of the change's answer: `{database, role, password,
 * host, port}`, `password` null for a role that has none. Null when no line
 * holds one, so that the page never shows an empty password as if it were
 * the database's.
 */
export function secretOf(answer) {
  const lines = Array.isArray(answer?.output) ? answer.output : [];
  for (const line of lines) {
    let found;
    try {
      found = JSON.parse(line);
    } catch {
      continue;
    }
    if (isObject(found) && typeof found.database === 'string' && typeof found.role === 'string' && 'password' in found) {
      return {
        database: found.database,
        role: found.role,
        password: typeof found.password === 'string' && found.password !== '' ? found.password : null,
        host: typeof found.host === 'string' ? found.host : null,
        port: Number.isInteger(found.port) ? found.port : PORT,
      };
    }
  }
  return null;
}

// ---- what each action is sent -----------------------------------------------

/**
 * The arguments of `databases.add` for a form: `{name, owner, account, locale,
 * site, way}`. The role is `owner`, and the account's own name for `socket`,
 * which sends `peer` and no owner; `owner` is left out when it is the name.
 */
export function addArgs({
  name, owner, account = '', locale, site = '', way,
}) {
  const args = { name };
  if (way === 'socket') args.peer = account;
  else if (owner !== name) args.owner = owner;
  args.locale = locale;
  if (site !== '') args.site = site;
  return args;
}

/** The arguments of `databases.allow` for `networks` and the certificate `certificate`. */
export const allowArgs = (name, networks, certificate) => ({ name, from: networks.join('\n'), certificate });

/** The arguments of `databases.deny` for one network, or every one when `network` is null. */
export const denyArgs = (name, network = null) => (network === null ? { name } : { name, from: network });

/** The arguments of `databases.remove`: a flag is there only when it is on. */
export function removeArgs(name, { role = false, force = false } = {}) {
  const args = { name };
  if (role) args.role = true;
  if (force) args.force = true;
  return args;
}

// ---- what the Add page says will happen -------------------------------------

/** Words for a list of networks: one, two, or how many. */
const networksWords = (networks) => (networks.length <= 3 ? networks.join(', ') : `${networks.length} networks`);

/** Whether `server` (`{listen, ssl, certificate}` of the last read) listens on the network and not on this machine only. */
export const listensOnNetwork = (server) => Boolean(server) && server.listen !== '' && !['localhost', '127.0.0.1', '::1'].includes(server.listen);

/**
 * Whether adding the way `network` with the certificate `certificate`
 * restarts PostgreSQL: it does unless the server listens on the network with
 * TLS and that certificate already.
 */
export const restarts = (server, certificate) => !(listensOnNetwork(server) && server.ssl === true && server.certificate === certificate);

/** `items` as the words of a sentence: "a", "a and b", "a, b and c". */
const listed = (items) => (items.length < 2 ? items.join('') : `${items.slice(0, -1).join(', ')} and ${items.at(-1)}`);

/** The networks that `server` (a read's) says port 5432 is open to: those of the firewall's block, none while it has none. */
const openNetworks = (server) => server?.firewall?.networks ?? [];

/**
 * What the firewall says of port 5432 as the `server` of a read has it:
 * open to the networks of its block only, closed to other machines, or
 * ruled by no block of its own while PostgreSQL listens on the network.
 */
export function portWords(server) {
  const open = openNetworks(server);
  if (open.length > 0) return `Port ${PORT} is open to ${listed(open)} only.`;
  if (listensOnNetwork(server)) {
    return `Port ${PORT} has no rule of its own: PostgreSQL listens on the network, so every address reaches it, and only pg_hba.conf keeps the others out.`;
  }
  return `Port ${PORT} is closed to other machines: PostgreSQL listens on this machine only.`;
}

/** What port 5432 is once `networks` are let in: opened to them and to the networks it is open to now, and to no other. */
function openedTo(server, networks) {
  const all = [...new Set([...openNetworks(server), ...networks])];
  return `opened to ${all.length === 0 ? 'the networks you name' : listed(all)} only`;
}

/** What the firewall will say of port 5432 once `networks` are let in. */
export const openedWords = (server, networks) => `Port ${PORT} is ${openedTo(server, networks)}.`;

/** The words around a rule that decides for port 5432 before the block does, as the firewall's file writes the rule: what comes before it and what after. */
export const EARLIER_LEAD = `A rule earlier in the firewall's rules decides for port ${PORT} first: `;
export const EARLIER_TAIL = '. Move or remove it on the Firewall page.';

/** What the Firewall row says: what the firewall does with port 5432 once this database is made. */
function firewallWords(away, server, networks) {
  if (away) return `Firewall: port ${PORT} is ${openedTo(server, networks)}.`;
  if (openNetworks(server).length > 0) {
    return `Firewall: nothing to change. Port ${PORT} stays open to ${listed(openNetworks(server))} only, and only programs on this server may sign in to this database.`;
  }
  return listensOnNetwork(server)
    ? 'Firewall: nothing to change. PostgreSQL listens on the network already, and only programs on this server may sign in to this database.'
    : `Firewall: port ${PORT} stays closed to other machines, since PostgreSQL listens on this machine only.`;
}

/**
 * The rows of What happens: `{icon, words, ok}`. `form` is `{name, owner,
 * account, way, locale, networks, certificate}`; `server` is the last read's,
 * or null; `locale` is the label of the text and sorting.
 */
export function planRows({
  name, owner, account, way, locale, networks, certificate,
}, server = null) {
  const role = way === 'socket' ? account : owner;
  const from = {
    local: 'with a password, from this server only',
    socket: `with no password, from the account ${account}, by the socket`,
    network: `with a password, from ${networksWords(networks)} over TLS`,
  }[way];
  const away = way === 'network';
  const restart = away && restarts(server, certificate);
  return [
    { icon: 'accounts', words: `Makes the PostgreSQL role ${role}, which signs in ${from}` },
    { icon: 'db', words: `Makes the database ${name}, owned by ${role}, ${locale}. Other roles may not connect to it.` },
    {
      icon: 'check',
      ok: true,
      words: firewallWords(away, server, networks),
    },
    ...(away && server?.firewall?.earlier ? [{ icon: 'warn', words: `${EARLIER_LEAD}${server.firewall.earlier}${EARLIER_TAIL}` }] : []),
    {
      icon: 'check',
      ok: true,
      words: restart ? 'Restart: PostgreSQL restarts once, and its open sessions end.' : 'Restart: none; the database is ready at once.',
    },
  ];
}
