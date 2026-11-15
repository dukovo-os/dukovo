// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A log query from the address to the read and back (design 5.1 to 5.5), and
// the records that come of it: the words a person typed made into a pattern
// that matches them as typed, the arguments of `logs.read`, the lines folded
// and merged, a saved line's text, and the events of a follow. Pure: the
// Logs page, Services, the chart view and Reliability all build on it.
//
// The address keeps `since` and `until` in Unix milliseconds, so that a link
// from a chart or a log line is exact. The read takes them in Unix seconds.

import {
  stampOf, timeOf, wallOf, windowOf,
} from './when-model.js';

/** The importance words of the filter, in its order, with the level each asks `log` for. */
export const PRIORITIES = Object.freeze([
  { id: 'error', label: 'Errors only' },
  { id: 'warning', label: 'Warnings and errors' },
  { id: 'info', label: 'Information' },
  { id: 'debug', label: 'Everything' },
]);

/** The zones the times can be shown in. */
export const ZONES = Object.freeze([
  { id: 'you', label: 'Your zone' },
  { id: 'server', label: 'The server\'s zone' },
  { id: 'utc', label: 'UTC' },
]);

/** The lines a page asks for, and the most a read gives. */
export const PAGE_LINES = 200;
export const MOST_LINES = 1000;

/** The most characters a pattern may have: the read's own limit. */
export const PATTERN_MAX = 200;

/** What the address says when it says nothing: the last hour's warnings and errors, from everywhere, in the person's zone. */
export const DEFAULT_QUERY = Object.freeze({
  when: '1h', since: null, until: null, service: '', kernel: false, priority: 'warning', q: '', zone: 'you', fold: false, lines: PAGE_LINES,
});

const WHENS = ['15m', '1h', '24h', '7d', 'today', 'custom'];
const ZONE_IDS = ZONES.map((zone) => zone.id);
const LEVELS = PRIORITIES.map((priority) => priority.id);

// Spark's rule for a service's name: letters, digits and "-_.@", and no leading dot.
const SERVICE = /^[A-Za-z0-9_@-][A-Za-z0-9._@-]{0,254}$/;
// The C0 and C1 controls and DEL, which a pattern may not hold.
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/;
const WHOLE = /^[1-9]\d{0,14}$/;

// ---- the words of a search ---------------------------------------------------

/** `words` as a pattern that matches them as typed: each character an extended pattern would read as its own is escaped. */
export const literal = (words) => String(words).replace(/[\\.[\]{}()*+?^$|]/g, '\\$&');

/**
 * Why `words` cannot be searched for, in words for the field, or null when
 * they can: no control characters, and a pattern of at most `PATTERN_MAX`
 * characters once they are escaped.
 */
export function wordsProblem(words) {
  const text = String(words).trim();
  if (CONTROL.test(text)) return 'The words cannot hold control characters.';
  if ([...literal(text)].length > PATTERN_MAX) return 'Those words are too long to search for. Use fewer.';
  return null;
}

/** The pattern the read searches for, from the query's words: null when there are none. */
export const grepOf = (q) => (q === '' ? null : literal(q));

// ---- the address ----------------------------------------------------------------

function paramsOf(source) {
  if (source instanceof URLSearchParams) return source;
  if (source instanceof URL) return source.searchParams;
  const text = String(source ?? '').split('#')[0];
  const mark = text.indexOf('?');
  if (mark >= 0) return new URLSearchParams(text.slice(mark + 1));
  return new URLSearchParams(text.startsWith('/') ? '' : text);
}

/** A Unix time in milliseconds from its digits, or undefined. */
const stamp = (text) => (WHOLE.test(text) ? Number(text) : undefined);

const word = (words) => (value) => (words.includes(value) ? value : undefined);
const set = (value) => (value === '1' ? true : undefined);
const count = (value) => {
  const found = /^\d{1,4}$/.test(value) ? Number(value) : 0;
  return found >= 1 && found <= MOST_LINES ? found : undefined;
};

/**
 * The query an address holds: `source` is the address, its query string, a
 * `URL` or `URLSearchParams`. Gives `{query, refused}`: the query, with the
 * default for each name that is missing or that holds what it may not, and
 * the names that were refused, so that the page can say so. It never throws,
 * and a name it does not know is not its business.
 */
export function queryOf(source) {
  const params = paramsOf(source);
  const query = { ...DEFAULT_QUERY };
  const refused = [];
  /** Sets `name` to what `parse` makes of its value; a value that `parse` gives undefined for is refused. */
  const read = (name, parse) => {
    const value = params.get(name);
    if (value === null) return;
    const found = parse(value);
    if (found === undefined) refused.push(name);
    else query[name] = found;
  };

  read('when', word(WHENS));
  if (query.when === 'custom') {
    // A custom range starts somewhere; it may be open at the end.
    read('since', stamp);
    if (query.since === null) {
      if (!refused.includes('since')) refused.push('since');
      query.when = DEFAULT_QUERY.when;
    } else {
      read('until', (value) => (stamp(value) > query.since ? stamp(value) : undefined));
    }
  }
  read('service', (value) => (SERVICE.test(value) ? value : undefined));
  read('kernel', set);
  if (query.service !== '' && query.kernel) {
    // Service and the machine itself are one choice, so the service wins.
    query.kernel = false;
    refused.push('kernel');
  }
  read('priority', word(LEVELS));
  read('q', (value) => (wordsProblem(value) === null ? value.trim() : undefined));
  read('zone', word(ZONE_IDS));
  read('fold', set);
  read('lines', count);
  return { query, refused };
}

/** The address of `query`: `/logs`, and the names that differ from the defaults, in the table's order. */
export function urlOf(query) {
  const params = new URLSearchParams();
  const differs = (name) => query[name] !== DEFAULT_QUERY[name];
  if (differs('when')) params.set('when', query.when);
  if (query.when === 'custom') {
    if (query.since !== null) params.set('since', String(query.since));
    if (query.until !== null) params.set('until', String(query.until));
  }
  if (query.service !== '') params.set('service', query.service);
  if (query.kernel) params.set('kernel', '1');
  if (differs('priority')) params.set('priority', query.priority);
  if (query.q !== '') params.set('q', query.q);
  if (differs('zone')) params.set('zone', query.zone);
  if (query.fold) params.set('fold', '1');
  if (differs('lines')) params.set('lines', String(query.lines));
  const text = params.toString();
  return text === '' ? '/logs' : `/logs?${text}`;
}

/**
 * Whether going from `before` to `after` asks the log again: the zone and the
 * folding only change how the lines are shown, except that `today` begins at
 * midnight in the zone shown.
 */
export const needsRead = (before, after) => ['when', 'since', 'until', 'service', 'kernel', 'priority', 'q', 'lines']
  .some((name) => before[name] !== after[name]) || (after.when === 'today' && before.zone !== after.zone);

/** The zone's name for the choice of `zone` ("you", "server" or "utc"): `you` and `server` are the names of the person's and the server's, and null comes back when the server's is not known. */
export function zoneId(choice, { you, server = null }) {
  if (choice === 'utc') return 'UTC';
  return choice === 'server' ? server : you;
}

// ---- the read -------------------------------------------------------------------

const seconds = (ms) => Math.floor(ms / 1000);

/**
 * The arguments of `logs.read` and `logs.read-all` for `query` at `now`
 * (milliseconds), with `zone` the name of the zone the times are shown in,
 * which `today` starts in. `since` and `until` are Unix seconds: a time is
 * read from the second it falls in, and the end is the first second after
 * it. Names come in the order of the action's table.
 */
export function argsOf(query, { now, zone, lines = query.lines } = {}) {
  const args = { lines };
  if (query.service !== '') args.service = query.service;
  if (query.kernel) args.kernel = true;
  args.priority = query.priority;
  const span = query.when === 'custom' ? { since: query.since, until: query.until } : windowOf(query.when, now, zone);
  if (span?.since != null) args.since = seconds(span.since);
  if (span?.until != null) args.until = Math.ceil(span.until / 1000);
  const grep = grepOf(query.q);
  if (grep !== null) args.grep = grep;
  return args;
}

/** The path of a read with `args`: `/api/1/logs` for the person's own lines, `/api/1/logs/all` for an unlocked administrator's whole log. */
export function readPath(args, { all = false } = {}) {
  const params = new URLSearchParams();
  for (const [name, value] of Object.entries(args)) params.set(name, String(value));
  return `/api/1/logs${all ? '/all' : ''}?${params}`;
}

/** The path of a follow of `query`: only what a read narrows by, not its times or its lines. */
export function followPath(query, { all = false } = {}) {
  const params = new URLSearchParams();
  if (query.service !== '') params.set('service', query.service);
  if (query.kernel) params.set('kernel', 'true');
  params.set('priority', query.priority);
  const grep = grepOf(query.q);
  if (grep !== null) params.set('grep', grep);
  if (all) params.set('all', 'true');
  return `/api/1/logs/follow?${params}`;
}

/**
 * The arguments that read the lines before `oldest`, the oldest record
 * shown: the same filters and start, ending at the second after its second
 * (log reads times to the second, and an end is before the time it names).
 * Null when `oldest` has no time.
 */
export function olderArgs(query, oldest, { now, zone, lines = query.lines } = {}) {
  const at = timeOf(oldest?.time);
  if (!at) return null;
  const args = argsOf(query, { now, zone, lines });
  args.until = Math.min(args.until ?? Infinity, at.seconds + 1);
  return args;
}

// ---- records -----------------------------------------------------------------------

/** The key that tells one record from another: the boot it was made in and its number there. */
export const keyOf = (record) => `${record.boot}:${record.seq}`;

const isCount = (value) => Number.isSafeInteger(value) && value >= 0;

/**
 * The record `value` is, as `log -o json` prints it, or null when it is not
 * one: `{time, boot, seq, level, from, name, process, user, audit, text}`,
 * with `process` and `user` null when it has none.
 */
export function recordOf(value) {
  if (value === null || typeof value !== 'object') return null;
  const {
    time, boot, seq, level, from, name, text,
  } = value;
  if (!timeOf(time) || typeof boot !== 'string' || !isCount(seq)) return null;
  if (typeof level !== 'string' || typeof from !== 'string' || typeof name !== 'string' || typeof text !== 'string') return null;
  return {
    time, boot, seq, level, from, name, process: isCount(value.process) ? value.process : null, user: isCount(value.user) ? value.user : null, audit: value.audit === true, text,
  };
}

/**
 * The records of a read's answer: `{records, skipped}`, with the ones that
 * are not records left out and counted, or null when the answer is not a
 * list at all.
 */
export function recordsOf(answer) {
  if (!Array.isArray(answer)) return null;
  const records = answer.map(recordOf).filter(Boolean);
  return { records: merge([], records).records, skipped: answer.length - records.length };
}

/** The order of the log: by time to the microsecond, then by boot and number. */
function compare(a, b) {
  const x = timeOf(a.time);
  const y = timeOf(b.time);
  const apart = (x.seconds - y.seconds) * 1_000_000 + x.micros - y.micros;
  if (apart !== 0) return apart;
  if (a.boot !== b.boot) return a.boot < b.boot ? -1 : 1;
  return a.seq - b.seq;
}

/**
 * `old` with the records of `added` that it does not have, oldest first,
 * told apart by `boot` and `seq` (a page read again, or a follow that
 * starts where a read ended, brings a record twice). Gives `{records,
 * added}`, `added` the count of records that were new; with none, `records`
 * is `old` itself.
 */
export function merge(old, added) {
  const seen = new Set(old.map(keyOf));
  const fresh = [];
  for (const record of added) {
    const key = keyOf(record);
    if (seen.has(key)) continue;
    seen.add(key);
    fresh.push(record);
  }
  if (fresh.length === 0) return { records: old, added: 0 };
  return { records: [...old, ...fresh].sort(compare), added: fresh.length };
}

const same = (a, b) => a.from === b.from && a.name === b.name && a.level === b.level && a.text === b.text;

/**
 * The rows of `records`: with `folded`, records that follow one another with
 * the same source, level and text are one row, each `{key, record, last,
 * count, records}`: the first record of the run, the last, how many, and all
 * of them. Without it, a row each.
 */
export function rowsOf(records, folded) {
  const rows = [];
  for (const record of records) {
    const before = rows[rows.length - 1];
    if (folded && before && same(before.record, record)) {
      before.last = record;
      before.count += 1;
      before.records.push(record);
    } else {
      rows.push({
        key: keyOf(record), record, last: record, count: 1, records: [record],
      });
    }
  }
  return rows;
}

/** The record's source as a filter says it: `{service}` for a service's line, `{kernel: true}` for the kernel's, and null for a program's, which no filter names. */
export function sourceOf(record) {
  if (record.from === 'kernel') return { kernel: true };
  return record.from === 'service' ? { service: record.name } : null;
}

/** `query` narrowed to the source of `record`, where it has one a filter can name; null where it has not. */
export function onlyFrom(query, record) {
  const source = sourceOf(record);
  return source && { ...query, service: source.service ?? '', kernel: source.kernel === true };
}

const AROUND = 5 * 60_000;

/** `query` for the five minutes either side of `record`: the source stays, the words go. */
export function aroundOf(query, record) {
  const at = timeOf(record.time);
  return {
    ...query, when: 'custom', since: at.seconds * 1000 - AROUND, until: at.seconds * 1000 + AROUND + 1000, q: '', lines: PAGE_LINES,
  };
}

/** `query` for the one second that `record` falls in, with as many lines as a read gives: where more than a page of lines share a second. */
export function secondOf(query, record) {
  const at = timeOf(record.time);
  return {
    ...query, when: 'custom', since: at.seconds * 1000, until: (at.seconds + 1) * 1000, lines: MOST_LINES,
  };
}

// ---- a saved line ----------------------------------------------------------------------

/** Who wrote a record, as a saved line says it: a service or program by name and its process, the kernel as `kernel`. */
const whoOf = (record) => (record.process === null || record.process === undefined || record.from === 'kernel' ? record.name : `${record.name}[${record.process}]`);

/**
 * The text of `record` as Save as file writes it, in `zone`: "2026-11-14
 * 10:30:00.123456 +0100 warning nginx[312]: text". The time is to the
 * microsecond, so that the line says the order `log` keeps; a line break in
 * the text is a space, so that a record is one line.
 */
export function lineOf(record, zone) {
  return `${stampOf(record.time, zone)} ${record.level} ${whoOf(record)}: ${record.text.replace(/\r\n|[\r\n]/g, ' ')}`;
}

// ---- the follow ----------------------------------------------------------------------------

/**
 * One event of the follow stream, from its name and its data text: `{kind:
 * 'command', shown}`, `{kind: 'record', record}`, `{kind: 'skipped',
 * count}` or `{kind: 'ended', why, error}`. Null for an event that is none
 * of these or whose data does not read, so that a stream with a bad event in
 * it still gives the rest.
 */
export function followEvent(type, data) {
  let value;
  try {
    value = JSON.parse(data);
  } catch {
    return null;
  }
  if (value === null || typeof value !== 'object') return null;
  if (type === 'command') return typeof value.shown === 'string' ? { kind: 'command', shown: value.shown } : null;
  if (type === 'record') {
    const record = recordOf(value);
    return record && { kind: 'record', record };
  }
  if (type === 'skipped') return Number.isSafeInteger(value.count) && value.count > 0 ? { kind: 'skipped', count: value.count } : null;
  if (type === 'ended') return typeof value.why === 'string' ? { kind: 'ended', why: value.why, error: typeof value.error === 'string' ? value.error : '' } : null;
  return null;
}

const ENDINGS = {
  locked: 'you locked the console',
  'unlock-ended': 'your unlock ended',
  'session-ended': 'this sign-in ended',
  replaced: 'you began following in another window',
  time: 'it had run for an hour',
  slow: 'the lines came faster than this page could take them',
  stopped: 'the server is stopping',
};

/** Why a follow ended, in a sentence, from its `ended` event: "Following stopped: you locked the console." */
export function endedWords({ why, error = '' }) {
  if (why === 'failed') {
    const said = error.trim();
    return said === '' ? 'Following stopped: the log command ended.' : `Following stopped: the log command ended. It said: ${said}`;
  }
  return ENDINGS[why] ? `Following stopped: ${ENDINGS[why]}.` : 'Following stopped.';
}

/** The sentence for a stream that broke with no `ended` event: the connection went. */
export const BROKEN_WORDS = 'Following stopped: the connection to the server was lost.';

/**
 * What a page says of the lines a person reads (design 5.3), or null when it
 * reads the whole log: `{words, unlock}`, where `unlock` is true when
 * unlocking the console would show more. `admin` is whether the account is an
 * administrator and `all` whether it reads the whole log now.
 */
export function whoWords(admin, all) {
  if (all) return null;
  const own = 'You see the lines of your own programs.';
  return admin ? { words: `${own} Unlock to read the whole log.`, unlock: true } : { words: `${own} The machine's log is for administrators.`, unlock: false };
}

// ---- saving and counting -------------------------------------------------------------------------------

/** The name of a saved file for the server `host` at `now` (milliseconds) in `zone`: "log-duke-nas-20261114-1030.txt". */
export function saveName(host, now, zone) {
  const { date, time } = wallOf(now, zone);
  const name = String(host).replace(/[^A-Za-z0-9._-]+/g, '-').replace(/^[-.]+|[-.]+$/g, '') || 'server';
  return `log-${name}-${date.replaceAll('-', '')}-${time.slice(0, 5).replace(':', '')}.txt`;
}

/** "12 new lines", and "1 new line". */
export const newLinesWords = (count) => `${count} new ${count === 1 ? 'line' : 'lines'}`;

/**
 * What tells a screen reader how many lines came while a follow runs: `add(count)`
 * is called as lines come, and `say(words)` is called with their count at
 * most once in `every` milliseconds, and not at all when none came, so that
 * a reader is not read every line (design 5.5). `stop()` forgets what is
 * waiting. `now`, `later` and `cancel` are the clock and the timer, which a
 * test replaces.
 */
export function makeAnnouncer(say, {
  every = 5000, now = () => Date.now(), later = (work, wait) => setTimeout(work, wait), cancel = (timer) => clearTimeout(timer),
} = {}) {
  let waiting = 0;
  let spoke = -Infinity;
  let timer = null;
  function flush() {
    timer = null;
    if (waiting === 0) return;
    say(newLinesWords(waiting));
    waiting = 0;
    spoke = now();
  }
  return {
    add(count) {
      if (!(count > 0)) return;
      waiting += count;
      if (timer !== null) return;
      const wait = spoke + every - now();
      if (wait <= 0) flush();
      else timer = later(flush, wait);
    },
    stop() {
      if (timer !== null) cancel(timer);
      timer = null;
      waiting = 0;
    },
  };
}
