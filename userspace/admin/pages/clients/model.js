// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the AI clients page says, as plain values (ADR 0690): the levels an
// area can be given and the scopes that give them, the command that adds a
// client, a client's row, a request's card and the lines of what the
// clients did. No document here, so that a test reads each.
//
// The areas are `GET /api/1/clients/areas`'s, each `{area, title, reads,
// reads_all, asks, who_gets_in}`: how many reads a person runs there, how
// many only an administrator runs, and how many changes a client may ask
// for. The levels follow from those counts, so the dialog offers no level
// that would give nothing.

import { lowerFirst } from '../../lib/action-model.js';
import { capital } from '../../lib/hold-words.js';
import { dayWords } from '../../lib/snapshot-words.js';
import { fingerprintGroups, servesOwnCertificate } from '../alerts/phone-setup-model.js';

/** The days a client can last that the dialog offers, the one it starts with, and the most authd takes. */
export const DAYS = [30, 90, 180, 366];
export const DAYS_DEFAULT = 90;

/** The words of `admin mcp` that no client is named, as authd holds it. */
const RESERVED = ['add', 'list', 'remove', 'help'];

/** What is wrong with `name` as a new client's, among the person's clients `taken`, or null. */
export function nameProblem(name, taken = []) {
  if (name === '') return 'Give the client a name.';
  if (!/^[a-z0-9-]{1,32}$/.test(name)) return 'A name is 1 to 32 lowercase letters, digits and hyphens.';
  if (RESERVED.includes(name)) return `No client is named ${name}: it is a word of the admin mcp command.`;
  if (taken.includes(name)) return `You already have a client named ${name}. Remove it first, or choose another name.`;
  return null;
}

/**
 * The levels `area` can be given, lowest first, each `{id, label}`: `off`,
 * then `read` when the area has reads a person runs, then `all` when it has
 * an administrator's reads or changes to ask for, named for what it adds.
 */
export function levelsOf(area) {
  const levels = [{ id: 'off', label: 'Off' }];
  if (area.reads > 0) levels.push({ id: 'read', label: 'Read' });
  if (area.reads_all > 0 || area.asks > 0) {
    const reads = area.reads_all > 0 ? 'read all' : area.reads > 0 ? 'read' : null;
    const label = [reads, area.asks > 0 ? 'ask' : null].filter(Boolean).join(' and ');
    levels.push({ id: 'all', label: capital(label) });
  }
  return levels;
}

/** The level of `area` that `wanted` comes to: itself when the area has it, else the nearest below it. */
function levelIn(area, wanted) {
  const ids = levelsOf(area).map((level) => level.id);
  if (ids.includes(wanted)) return wanted;
  return wanted === 'all' && ids.includes('read') ? 'read' : 'off';
}

/**
 * The level of every area for a start the dialog offers: `read` is every
 * read a person runs, and `all` the most each area has.
 */
export function chosenFor(areas, start) {
  return Object.fromEntries(areas.map((area) => {
    const ids = levelsOf(area).map((level) => level.id);
    return [area.area, start === 'all' ? ids.at(-1) : levelIn(area, 'read')];
  }));
}

/**
 * The scopes that give each area the level `chosen` has for it, in the
 * fewest words: `*` when every area is at its most, `*:read` when every
 * area with something for a person to read is read, and the areas by name
 * for the rest. Empty when nothing is chosen.
 */
export function scopesOf(areas, chosen) {
  const at = (area) => levelIn(area, chosen[area.area] ?? 'off');
  if (areas.length > 0 && areas.every((area) => at(area) === levelsOf(area).at(-1).id)) return ['*'];
  const readable = areas.filter((area) => area.reads > 0);
  const everyRead = readable.length > 0 && readable.every((area) => at(area) !== 'off');
  const named = areas.flatMap((area) => {
    if (at(area) === 'all') return [area.area];
    return at(area) === 'read' && !everyRead ? [`${area.area}:read`] : [];
  });
  return everyRead ? ['*:read', ...named] : named;
}

/** The level each area has for a client with `scopes`, by the area's name. */
export function levelsFor(areas, scopes) {
  const whole = (area) => scopes.includes('*') || scopes.includes(area.area);
  const read = (area) => whole(area) || scopes.includes('*:read') || scopes.includes(`${area.area}:read`);
  return Object.fromEntries(areas.map((area) => [area.area, levelIn(area, whole(area) ? 'all' : read(area) ? 'read' : 'off')]));
}

/**
 * What a client with `scopes` reaches, in the words of a row: `reads` and
 * `asks`, each `{every, titles}`, where `every` says that it is all there
 * is to read, or all that a client may ask for.
 */
export function reachOf(areas, scopes) {
  const levels = levelsFor(areas, scopes);
  const readable = areas.filter((area) => area.reads + area.reads_all > 0);
  const read = readable.filter((area) => levels[area.area] !== 'off');
  const askable = areas.filter((area) => area.asks > 0);
  const asked = askable.filter((area) => levels[area.area] === 'all');
  const told = (some, all) => ({ every: all.length > 0 && some.length === all.length, titles: some.map((area) => area.title) });
  return { reads: told(read, readable), asks: told(asked, askable) };
}

/** A scope as a shell takes it: one with a star is quoted, so that the shell does not read it as a pattern. */
const quoted = (scope) => (scope.includes('*') ? `'${scope}'` : scope);

/** The longest line of a command before it goes on under itself. */
const LINE = 72;

/**
 * `admin mcp add` for a client: the stars each as a scope of their own and
 * the named areas as one list, broken with a backslash where the line
 * grows long.
 */
export function addCommand({ name, way, scopes, days }) {
  const stars = scopes.filter((scope) => scope.includes('*'));
  const named = scopes.filter((scope) => !scope.includes('*'));
  const parts = [
    `admin mcp add ${name === '' ? 'NAME' : name}`,
    way === 'https' && '--https',
    ...stars.map((scope) => `--scope ${quoted(scope)}`),
    named.length > 0 && `--scope ${named.join(',')}`,
    `--days ${days}`,
  ].filter(Boolean);
  const lines = [];
  for (const part of parts) {
    const last = lines.at(-1);
    if (last !== undefined && `${last} ${part}`.length <= LINE) lines[lines.length - 1] = `${last} ${part}`;
    else lines.push(part);
  }
  return lines.join(' \\\n    ');
}

/** The day a client added now for `days` days ends, with the days: "In 90 days · Feb 22, 2027". */
export function endsWords(days, zone, now = Date.now()) {
  return `In ${days} days · ${dayWords(now + days * 86_400_000, zone, now)}`;
}

/** How a client connects, in the words of its row. */
const WAYS = { ssh: 'SSH', https: 'HTTPS, token' };

/**
 * The rows of the table for `clients`, as authd lists them: each `{owner,
 * name, way, reads, asks, used, from, ends, ended}`, ready to show. `time`
 * is the view's; a client that was never used says so.
 */
export function clientRows(clients, areas, time, now = Date.now()) {
  return clients.map((client) => {
    const day = dayWords(client.until * 1000, time.zone, now);
    return {
      owner: client.owner,
      name: client.name,
      way: WAYS[client.way] ?? client.way,
      ...reachOf(areas, client.scopes ?? []),
      used: client.used > 0 ? capital(time.relative(client.used)) : 'Never',
      from: client.used > 0 && client.from ? `from ${client.from}` : null,
      ends: client.ended ? `Ended ${day}` : day,
      ended: client.ended === true,
    };
  });
}

/** A client as a sentence names it: its name, with whose it is when it is not `mine`'s own. */
export function clientName(owner, name, mine) {
  return owner === mine ? name : `${owner}'s ${name}`;
}

/** How long a request asked at `asked` has waited: "Waiting 2 minutes". */
export function waitingWords(asked, now = Date.now()) {
  const minutes = Math.floor((now / 1000 - asked) / 60);
  if (minutes < 1) return 'Waiting less than a minute';
  return `Waiting ${minutes} ${minutes === 1 ? 'minute' : 'minutes'}`;
}

/**
 * The words of the card of `request`, one of `GET
 * /api/1/clients/requests`'s: its title, how long it has waited, who asked
 * and when it lapses, the reason, the command and the files.
 */
export function cardWords(request, time, now = Date.now()) {
  const files = Array.isArray(request.files) ? request.files : [];
  return {
    id: request.request,
    title: request.title,
    waiting: waitingWords(request.asked, now),
    asked: `Asked by ${request.client}, ${request.owner}'s client, at ${time.when(request.asked)} · request ${request.request} · lapses at ${time.when(request.lapses)} if nobody answers`,
    reason: request.reason,
    command: typeof request.command === 'string' ? request.command : null,
    files: files.length === 0 ? 'None' : files.join(', '),
  };
}

/** What `POST /api/1/check` said a change would do, in the card's words; `undefined` is a check that failed. */
export function changesWords(changed) {
  if (changed === true) return 'Yes.';
  if (changed === false) return 'No. It is already so, and a yes changes nothing.';
  return changed === null ? 'The console cannot tell before it runs.' : 'The console could not find out.';
}

/** What became of a request, by its state: the words under its line and the colour of its mark. */
function outcomeOf(entry, time) {
  const at = entry.ended ? ` at ${time.when(entry.ended)}` : '';
  switch (entry.state) {
    case 'waiting': return { detail: 'Waiting for a yes', kind: 'info' };
    case 'approved': return { detail: `Approved by ${entry.by} · it runs now`, kind: 'ok' };
    case 'done': return { detail: `Approved by ${entry.by}${at} · done`, kind: 'ok' };
    case 'task': return { detail: `Approved by ${entry.by}${at} · task ${entry.task}`, kind: 'ok' };
    case 'failed': return { detail: `Approved by ${entry.by}${at} · it failed, and the client was told why`, kind: 'bad' };
    case 'refused': return { detail: `Refused by ${entry.by}`, kind: 'warn' };
    default: return { detail: 'Lapsed: nobody said yes, or the client was removed', kind: null };
  }
}

/**
 * The lines of What they did for `entries`, `GET /api/1/clients/history`'s,
 * the newest first: each `{at, title, detail, kind, task}`. `mine` is the
 * account that looks, whose own clients go by their names alone.
 */
export function historyLines(entries, areas, time, mine) {
  const titles = new Map(areas.map((area) => [area.area, area.title]));
  return entries.map((entry) => {
    const who = clientName(entry.owner, entry.client, mine);
    if (entry.kind === 'read') {
      return {
        at: time.when(entry.at),
        title: `${who} read ${entry.count} ${entry.count === 1 ? 'thing' : 'things'}`,
        detail: (entry.areas ?? []).map((area) => titles.get(area) ?? area).join(', '),
        kind: null,
        task: null,
      };
    }
    return {
      at: time.when(entry.at),
      title: `${who} asked to ${lowerFirst(entry.title ?? entry.action)}`,
      ...outcomeOf(entry, time),
      task: entry.state === 'task' ? entry.task : null,
    };
  });
}

/** The badge of the navigation for `waiting` requests, or null for none. */
export function badgeOf(waiting) {
  return waiting > 0 ? { count: waiting, label: 'waiting for a yes', kind: 'info' } : null;
}

/** The name SSH reaches the machine by, from the name the browser does: an IPv6 address without its brackets. */
export const sshHost = (hostname) => hostname.replace(/^\[(.*)\]$/, '$1');

/**
 * What the settings of a client that connects over SSH hold:
 * the command that its owner's SSH sign-in runs. `server` is the machine's
 * name, which the settings call it by, and `host` the name SSH reaches it
 * by.
 */
export function sshSettings({ server, host, owner, name }) {
  return JSON.stringify({ mcpServers: { [server]: { command: 'ssh', args: [`${owner}@${host}`, 'admin', 'mcp', name] } } }, null, 2);
}

/** The command that tries a client that connects over SSH from a terminal. */
export function sshCheck({ host, owner, name }) {
  return `ssh ${owner}@${host} admin mcp ${name} --check`;
}

/** What the settings of a client that connects over HTTPS hold: the console's address, `origin`, and the token. */
export function httpsSettings({ server, origin, token }) {
  return JSON.stringify({
    mcpServers: { [server]: { type: 'http', url: `${origin}/mcp`, headers: { Authorization: `Bearer ${token}` } } },
  }, null, 2);
}

/**
 * What is known of the certificate the console serves, from `GET
 * /api/1/console/about` and the machine's own certificate, either of them
 * null when its read failed: `own` is true when the console serves the
 * machine's own, which no client trusts until it is told to, false when
 * it serves another's, and null when the console cannot tell; `fingerprint`
 * is its SHA-256, eight bytes a group.
 */
export function certificateOf(about, machine) {
  const known = typeof about?.fingerprint === 'string' && typeof machine?.fingerprint_sha256 === 'string';
  return {
    own: known ? servesOwnCertificate(about.fingerprint, machine.fingerprint_sha256) : null,
    fingerprint: fingerprintGroups(about?.fingerprint),
  };
}
