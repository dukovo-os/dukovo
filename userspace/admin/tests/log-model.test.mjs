// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Logs page's query and records (design 5.1 to 5.5): the address and back,
// the arguments of the read, the words of a search made literal, paging,
// merging, folding, a saved line in each zone, and the events of a follow.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  aroundOf, argsOf, BROKEN_WORDS, makeAnnouncer, newLinesWords, saveName, DEFAULT_QUERY, endedWords, followEvent, followPath, grepOf, keyOf, lineOf, literal, merge, needsRead,
  olderArgs, onlyFrom, queryOf, readPath, recordOf, recordsOf, rowsOf, secondOf, sourceOf, urlOf, whoWords, wordsProblem, zoneId,
} from '../lib/log-model.js';

const PRAGUE = 'Europe/Prague';
const utc = (...parts) => Date.UTC(...parts);

/** A record as `log -o json` prints it, with `over` changed. */
const record = (over = {}) => ({
  time: '2026-11-14T09:30:00.123456Z', boot: 'b1', seq: 1, level: 'warning', from: 'service', name: 'nginx', process: 312, user: 0, text: 'upstream timed out', ...over,
});
/** The query of an address. */
const query = (address) => queryOf(address).query;

// ---- literal ------------------------------------------------------------------

test('literal escapes each character an extended pattern would read as its own', () => {
  // The design names twelve; the list it gives is fourteen characters, and each is escaped.
  for (const special of ['\\', '.', '[', ']', '{', '}', '(', ')', '*', '+', '?', '^', '$', '|']) {
    assert.equal(literal(special), `\\${special}`, special);
    assert.equal(literal(`a${special}b`), `a\\${special}b`, special);
  }
});

test('literal leaves everything else as it was typed, and escapes a backslash once', () => {
  assert.equal(literal('timed out'), 'timed out');
  assert.equal(literal(''), '');
  assert.equal(literal('příliš žluťoučký 日本語 -/:;<>=!@#%&~`"\''), 'příliš žluťoučký 日本語 -/:;<>=!@#%&~`"\'');
  assert.equal(literal('a\\b'), 'a\\\\b');
  assert.equal(literal('\\.'), '\\\\\\.');
  assert.equal(literal('timeout (30s) [retry] x*y+z?'), 'timeout \\(30s\\) \\[retry\\] x\\*y\\+z\\?');
  assert.equal(literal('^start$'), '\\^start\\$');
  assert.equal(literal('a|b'), 'a\\|b');
  assert.equal(literal('{3}'), '\\{3\\}');
});

test('a pattern from literal matches the words that made it, and only those', () => {
  for (const words of ['a.b', '[x]', '(1+1)*2', 'x{2}', '^$', 'a|b', 'C:\\dir', '50% off?', 'plain text']) {
    const pattern = new RegExp(`^${literal(words)}$`);
    assert.ok(pattern.test(words), words);
  }
  assert.equal(new RegExp(literal('a.b')).test('axb'), false);
  assert.equal(new RegExp(literal('a|b')).test('a'), false);
  assert.equal(new RegExp(literal('x*')).test('xxx'), false);
  assert.equal(new RegExp(literal('[ab]')).test('a'), false);
});

test('words that cannot be searched for are refused with a reason', () => {
  assert.equal(wordsProblem('timed out'), null);
  assert.equal(wordsProblem(''), null);
  assert.equal(wordsProblem('  padded  '), null);
  assert.match(wordsProblem('a\tb'), /control characters/);
  assert.match(wordsProblem('a\u0000b'), /control characters/);
  assert.match(wordsProblem('a\nb'), /control characters/);
  assert.match(wordsProblem('a\u007fb'), /control characters/);
  assert.match(wordsProblem('a\u0085b'), /control characters/);
  // The limit is on the pattern the read gets: 200 characters once each special one has its backslash.
  assert.equal(wordsProblem('x'.repeat(200)), null);
  assert.match(wordsProblem('x'.repeat(201)), /too long/);
  assert.equal(wordsProblem('.'.repeat(100)), null);
  assert.match(wordsProblem('.'.repeat(101)), /too long/);
  assert.equal(wordsProblem('ž'.repeat(200)), null);
});

test('the pattern of a query is its words made literal, and none without words', () => {
  assert.equal(grepOf(''), null);
  assert.equal(grepOf('a.b'), 'a\\.b');
});

// ---- the address -----------------------------------------------------------------------

test('an address with no query is the last hour of warnings and errors', () => {
  const { query: found, refused } = queryOf('/logs');
  assert.deepEqual(found, DEFAULT_QUERY);
  assert.deepEqual(refused, []);
  assert.equal(DEFAULT_QUERY.when, '1h');
  assert.equal(DEFAULT_QUERY.priority, 'warning');
  assert.equal(DEFAULT_QUERY.zone, 'you');
  assert.equal(urlOf(DEFAULT_QUERY), '/logs');
});

test('each filter goes into the address and comes back', () => {
  const cases = [
    { when: '15m' }, { when: '24h' }, { when: '7d' }, { when: 'today' },
    { when: 'custom', since: 1_763_100_000_000, until: 1_763_103_600_000 },
    { when: 'custom', since: 1_763_100_000_000 },
    { service: 'nginx' }, { service: 'my-app_2.service@x' },
    { kernel: true },
    { priority: 'error' }, { priority: 'info' }, { priority: 'debug' },
    { q: 'timed out' }, { q: 'a.b (c) [d] 100% & more=less?' }, { q: 'příliš žluťoučký 日本語' }, { q: 'x'.repeat(200) },
    { zone: 'server' }, { zone: 'utc' },
    { fold: true },
    { lines: 1000 }, { lines: 1 }, { lines: 20 },
  ];
  for (const change of cases) {
    const made = { ...DEFAULT_QUERY, ...change };
    const address = urlOf(made);
    const back = queryOf(address);
    assert.deepEqual(back.query, made, address);
    assert.deepEqual(back.refused, [], address);
    // And the address a person copies is the address again.
    assert.equal(urlOf(back.query), address);
  }
});

test('every filter at once comes back, with the names in the order of the table', () => {
  const made = {
    when: 'custom', since: 1_763_100_000_000, until: 1_763_103_600_000, service: 'nginx', kernel: false, priority: 'info', q: 'timed out', zone: 'utc', fold: true, lines: 500,
  };
  const address = urlOf(made);
  assert.equal(address, '/logs?when=custom&since=1763100000000&until=1763103600000&service=nginx&priority=info&q=timed+out&zone=utc&fold=1&lines=500');
  assert.deepEqual(queryOf(address).query, made);
});

test('the address holds only what differs from the defaults', () => {
  assert.equal(urlOf({ ...DEFAULT_QUERY, when: '1h' }), '/logs');
  assert.equal(urlOf({ ...DEFAULT_QUERY, priority: 'warning', zone: 'you', lines: 200 }), '/logs');
  assert.equal(urlOf({ ...DEFAULT_QUERY, priority: 'error' }), '/logs?priority=error');
  // The times of a range that is not custom are not the address's.
  assert.equal(urlOf({ ...DEFAULT_QUERY, when: '24h', since: 5, until: 9 }), '/logs?when=24h');
});

test('the query is read from an address, a query string, a URL or its parameters', () => {
  const want = { ...DEFAULT_QUERY, when: '24h', priority: 'error' };
  assert.deepEqual(query('/logs?when=24h&priority=error'), want);
  assert.deepEqual(query('?when=24h&priority=error'), want);
  assert.deepEqual(query('when=24h&priority=error'), want);
  assert.deepEqual(query('/logs?when=24h&priority=error#top'), want);
  assert.deepEqual(query(new URL('https://duke-nas/logs?when=24h&priority=error')), want);
  assert.deepEqual(query(new URLSearchParams('when=24h&priority=error')), want);
  assert.deepEqual(query(undefined), DEFAULT_QUERY);
  assert.deepEqual(query(''), DEFAULT_QUERY);
  assert.deepEqual(query('/logs'), DEFAULT_QUERY);
});

test('a name that is not the query\'s is left alone, and the first of a name that comes twice is read', () => {
  assert.deepEqual(queryOf('/logs?utm=1&when=24h'), { query: { ...DEFAULT_QUERY, when: '24h' }, refused: [] });
  assert.equal(query('/logs?priority=error&priority=info').priority, 'error');
});

test('a value the query may not hold is refused by name, and the default stands', () => {
  const refuses = (address, ...names) => {
    const { query: found, refused } = queryOf(address);
    assert.deepEqual(refused, names, address);
    return found;
  };
  assert.equal(refuses('?when=yesterday', 'when').when, '1h');
  assert.equal(refuses('?when=custom', 'since').when, '1h');
  assert.equal(refuses('?when=custom&since=soon', 'since').when, '1h');
  assert.equal(refuses('?when=custom&since=-5', 'since').when, '1h');
  assert.equal(refuses('?when=custom&since=1.5', 'since').when, '1h');
  assert.equal(refuses('?when=custom&since=0', 'since').when, '1h');
  assert.equal(refuses('?when=custom&since=1000&until=1000', 'until').until, null);
  assert.equal(refuses('?when=custom&since=1000&until=900', 'until').until, null);
  assert.equal(refuses('?when=custom&since=1000&until=later', 'until').until, null);
  assert.equal(refuses('?when=custom&since=1000&until=2000').until, 2000);
  assert.equal(refuses('?service=../x', 'service').service, '');
  assert.equal(refuses('?service=.hidden', 'service').service, '');
  assert.equal(refuses('?service=a%20b', 'service').service, '');
  assert.equal(refuses('?service=a/b', 'service').service, '');
  assert.equal(refuses(`?service=${'a'.repeat(256)}`, 'service').service, '');
  assert.equal(refuses('?service=', 'service').service, '');
  assert.equal(refuses('?kernel=0', 'kernel').kernel, false);
  assert.equal(refuses('?kernel=true', 'kernel').kernel, false);
  assert.equal(refuses('?priority=loud', 'priority').priority, 'warning');
  assert.equal(refuses('?priority=ERROR', 'priority').priority, 'warning');
  assert.equal(refuses('?q=a%00b', 'q').q, '');
  assert.equal(refuses('?q=a%0Ab', 'q').q, '');
  assert.equal(refuses(`?q=${'x'.repeat(201)}`, 'q').q, '');
  assert.equal(refuses('?zone=moon', 'zone').zone, 'you');
  assert.equal(refuses('?fold=yes', 'fold').fold, false);
  assert.equal(refuses('?fold=0', 'fold').fold, false);
  assert.equal(refuses('?lines=0', 'lines').lines, 200);
  assert.equal(refuses('?lines=1001', 'lines').lines, 200);
  assert.equal(refuses('?lines=ten', 'lines').lines, 200);
  assert.equal(refuses('?lines=-1', 'lines').lines, 200);
  assert.equal(refuses('?lines=12345', 'lines').lines, 200);
});

test('a refusal leaves the rest of the query as it was given', () => {
  const { query: found, refused } = queryOf('?when=24h&priority=loud&service=nginx&q=a%00b&fold=1');
  assert.deepEqual(refused, ['priority', 'q']);
  assert.deepEqual(found, { ...DEFAULT_QUERY, when: '24h', service: 'nginx', fold: true });
});

test('a service and the machine itself never come together: the service stays', () => {
  const { query: found, refused } = queryOf('?service=nginx&kernel=1');
  assert.equal(found.service, 'nginx');
  assert.equal(found.kernel, false);
  assert.deepEqual(refused, ['kernel']);
  assert.equal(query('?kernel=1').kernel, true);
});

test('times in the address matter only to a custom range', () => {
  assert.deepEqual(queryOf('?when=24h&since=1000&until=2000'), { query: { ...DEFAULT_QUERY, when: '24h' }, refused: [] });
});

test('words are trimmed, and spaces and plus signs in the address are read as the person typed them', () => {
  assert.equal(query('?q=%20%20timed+out%20').q, 'timed out');
  assert.equal(query('?q=a%2Bb').q, 'a+b');
  assert.equal(query(urlOf({ ...DEFAULT_QUERY, q: 'a+b c' })).q, 'a+b c');
  assert.equal(query('?q=%20%20').q, '');
});

test('only a change of what is read asks the log again', () => {
  const base = DEFAULT_QUERY;
  for (const change of [{ when: '24h' }, { since: 5 }, { until: 5 }, { service: 'nginx' }, { kernel: true }, { priority: 'info' }, { q: 'x' }, { lines: 500 }]) {
    assert.equal(needsRead(base, { ...base, ...change }), true, JSON.stringify(change));
  }
  for (const change of [{ zone: 'utc' }, { zone: 'server' }, { fold: true }]) {
    assert.equal(needsRead(base, { ...base, ...change }), false, JSON.stringify(change));
  }
  assert.equal(needsRead(base, { ...base }), false);
  // Today begins at midnight in the zone shown, so a zone changes what it asks for.
  const today = { ...base, when: 'today' };
  assert.equal(needsRead(today, { ...today, zone: 'utc' }), true);
  assert.equal(needsRead({ ...today, zone: 'utc' }, { ...today, zone: 'you' }), true);
  assert.equal(needsRead(today, { ...today, fold: true }), false);
});

test('the zone a choice names: the person\'s, the server\'s (when known) or UTC', () => {
  const names = { you: 'America/New_York', server: 'Europe/Prague' };
  assert.equal(zoneId('you', names), 'America/New_York');
  assert.equal(zoneId('server', names), 'Europe/Prague');
  assert.equal(zoneId('utc', names), 'UTC');
  assert.equal(zoneId('server', { you: 'America/New_York' }), null);
});

// ---- the read ------------------------------------------------------------------------------

test('a read of the last hour asks for its warnings and errors since an hour ago, in Unix seconds', () => {
  const now = utc(2026, 10, 14, 9, 30, 0, 750);
  assert.deepEqual(argsOf(DEFAULT_QUERY, { now, zone: PRAGUE }), { lines: 200, priority: 'warning', since: Math.floor(now / 1000) - 3600 });
});

test('each range gives its start in seconds, and none has an end', () => {
  const now = utc(2026, 10, 14, 9, 30, 0);
  const since = (when) => argsOf({ ...DEFAULT_QUERY, when }, { now, zone: PRAGUE });
  assert.equal(since('15m').since, now / 1000 - 900);
  assert.equal(since('1h').since, now / 1000 - 3600);
  assert.equal(since('24h').since, now / 1000 - 86_400);
  assert.equal(since('7d').since, now / 1000 - 604_800);
  assert.equal(since('7d').until, undefined);
  // 09:30 UTC is 10:30 in Prague: its midnight was 23:00 UTC the day before.
  assert.equal(since('today').since, utc(2026, 10, 13, 23) / 1000);
  assert.equal(argsOf({ ...DEFAULT_QUERY, when: 'today' }, { now, zone: 'America/New_York' }).since, utc(2026, 10, 14, 5) / 1000);
});

test('a custom range gives its own times: the start from its second, the end after it', () => {
  const custom = { ...DEFAULT_QUERY, when: 'custom', since: utc(2026, 10, 14, 9, 0, 0, 400), until: utc(2026, 10, 14, 9, 30, 0, 400) };
  const args = argsOf(custom, { now: Date.now(), zone: PRAGUE });
  assert.equal(args.since, utc(2026, 10, 14, 9) / 1000);
  assert.equal(args.until, utc(2026, 10, 14, 9, 30, 1) / 1000);
  const whole = argsOf({ ...custom, since: utc(2026, 10, 14, 9), until: utc(2026, 10, 14, 9, 30) }, { now: 0, zone: PRAGUE });
  assert.equal(whole.since, utc(2026, 10, 14, 9) / 1000);
  assert.equal(whole.until, utc(2026, 10, 14, 9, 30) / 1000);
  assert.equal(argsOf({ ...custom, until: null }, { now: 0, zone: PRAGUE }).until, undefined);
});

test('the filters become arguments, in the order of the action\'s table', () => {
  const now = utc(2026, 10, 14, 9, 30, 0);
  const full = argsOf({
    ...DEFAULT_QUERY, when: 'custom', since: 2_000_000, until: 3_000_000, service: 'nginx', priority: 'info', q: 'timed out (30s)', lines: 20,
  }, { now, zone: 'UTC' });
  assert.deepEqual(full, {
    lines: 20, service: 'nginx', priority: 'info', since: 2000, until: 3000, grep: 'timed out \\(30s\\)',
  });
  assert.deepEqual(Object.keys(full), ['lines', 'service', 'priority', 'since', 'until', 'grep']);
  assert.deepEqual(argsOf({ ...DEFAULT_QUERY, kernel: true, priority: 'error' }, { now, zone: 'UTC' }), { lines: 200, kernel: true, priority: 'error', since: now / 1000 - 3600 });
  assert.deepEqual(argsOf(DEFAULT_QUERY, { now, zone: 'UTC', lines: 1000 }).lines, 1000);
});

test('the path of a read is the person\'s lines, or the whole log, with the arguments in its query', () => {
  const args = { lines: 20, service: 'nginx', priority: 'warning', since: 1_763_100_000, grep: 'timed\\ out' };
  assert.equal(readPath(args), '/api/1/logs?lines=20&service=nginx&priority=warning&since=1763100000&grep=timed%5C+out');
  assert.equal(readPath(args, { all: true }), '/api/1/logs/all?lines=20&service=nginx&priority=warning&since=1763100000&grep=timed%5C+out');
  assert.equal(readPath({ lines: 200, kernel: true, priority: 'error' }), '/api/1/logs?lines=200&kernel=true&priority=error');
  const back = new URLSearchParams(readPath({ lines: 5, grep: 'a&b=c d%' }).split('?')[1]);
  assert.equal(back.get('grep'), 'a&b=c d%');
});

test('a follow asks for what a read narrows by, and not for times or lines', () => {
  assert.equal(followPath(DEFAULT_QUERY), '/api/1/logs/follow?priority=warning');
  assert.equal(followPath({ ...DEFAULT_QUERY, service: 'nginx', priority: 'info', q: 'a.b' }, { all: true }), '/api/1/logs/follow?service=nginx&priority=info&grep=a%5C.b&all=true');
  assert.equal(followPath({ ...DEFAULT_QUERY, kernel: true, when: '7d', lines: 1000 }), '/api/1/logs/follow?kernel=true&priority=warning');
});

// ---- Show older ------------------------------------------------------------------------------------

test('the page before the oldest line ends at the second after the oldest line\'s second', () => {
  const now = utc(2026, 10, 14, 9, 30, 0);
  const found = olderArgs(DEFAULT_QUERY, record({ time: '2026-11-14T09:12:07.123456Z' }), { now, zone: PRAGUE });
  assert.equal(found.until, utc(2026, 10, 14, 9, 12, 8) / 1000);
  // A record made in the last microsecond of its second is still before the end.
  assert.equal(olderArgs(DEFAULT_QUERY, record({ time: '2026-11-14T09:12:07.999999Z' }), { now, zone: PRAGUE }).until, utc(2026, 10, 14, 9, 12, 8) / 1000);
  assert.equal(olderArgs(DEFAULT_QUERY, record({ time: '2026-11-14T09:12:07.000000Z' }), { now, zone: PRAGUE }).until, utc(2026, 10, 14, 9, 12, 8) / 1000);
});

test('the page before keeps the filters, the start and the size of the page', () => {
  const now = utc(2026, 10, 14, 9, 30, 0);
  const made = {
    ...DEFAULT_QUERY, when: '24h', service: 'nginx', priority: 'error', q: 'x.y', lines: 50,
  };
  const found = olderArgs(made, record({ time: '2026-11-14T09:12:07.5Z' }), { now, zone: PRAGUE });
  assert.deepEqual(found, {
    lines: 50, service: 'nginx', priority: 'error', since: now / 1000 - 86_400, until: utc(2026, 10, 14, 9, 12, 8) / 1000, grep: 'x\\.y',
  });
  assert.equal(olderArgs(made, record(), { now, zone: PRAGUE, lines: 200 }).lines, 200);
});

test('a custom range\'s own end is not passed by the page before', () => {
  const made = { ...DEFAULT_QUERY, when: 'custom', since: utc(2026, 10, 14, 9), until: utc(2026, 10, 14, 9, 5) };
  const found = olderArgs(made, record({ time: '2026-11-14T09:30:00.000000Z' }), { now: 0, zone: PRAGUE });
  assert.equal(found.until, utc(2026, 10, 14, 9, 5) / 1000);
});

test('a record with no time has no page before it', () => {
  assert.equal(olderArgs(DEFAULT_QUERY, record({ time: 'soon' }), { now: 0, zone: PRAGUE }), null);
  assert.equal(olderArgs(DEFAULT_QUERY, undefined, { now: 0, zone: PRAGUE }), null);
});

// ---- records ------------------------------------------------------------------------------------------------

test('a record is read as log prints it, and a value that is not one is not', () => {
  assert.deepEqual(recordOf(record()), {
    time: '2026-11-14T09:30:00.123456Z', boot: 'b1', seq: 1, level: 'warning', from: 'service', name: 'nginx', process: 312, user: 0, audit: false, text: 'upstream timed out',
  });
  const bare = { time: '2026-11-14T09:30:00.123456Z', boot: 'b1', seq: 7, level: 'info', from: 'kernel', name: 'kernel', text: 'x' };
  assert.deepEqual(recordOf(bare), { ...bare, process: null, user: null, audit: false });
  assert.equal(recordOf({ ...bare, audit: true }).audit, true);
  assert.equal(recordOf(record({ text: '' })).text, '');
  for (const bad of [null, undefined, 5, 'x', [], {}, record({ time: 'now' }), record({ time: 5 }), record({ boot: 3 }), record({ seq: -1 }), record({ seq: 1.5 }), record({ seq: '1' }),
    record({ level: null }), record({ from: 5 }), record({ name: undefined }), record({ text: 5 })]) {
    assert.equal(recordOf(bad), null, JSON.stringify(bad));
  }
  // A process that is no number is no process.
  assert.equal(recordOf(record({ process: 'x', user: -3 })).process, null);
  assert.equal(recordOf(record({ process: 'x', user: -3 })).user, null);
});

test('the records of an answer come oldest first, once each, with the bad ones counted', () => {
  const answer = [record({ seq: 3, time: '2026-11-14T09:30:03.000000Z' }), 'junk', record({ seq: 1, time: '2026-11-14T09:30:01.000000Z' }), { time: 5 }, record({ seq: 1, time: '2026-11-14T09:30:01.000000Z' })];
  const { records, skipped } = recordsOf(answer);
  assert.deepEqual(records.map((one) => one.seq), [1, 3]);
  assert.equal(skipped, 2);
  assert.deepEqual(recordsOf([]), { records: [], skipped: 0 });
  assert.equal(recordsOf({ lines: [] }), null);
  assert.equal(recordsOf(null), null);
  assert.equal(recordsOf('[]'), null);
});

test('merge drops a record the list has, told apart by its boot and its number', () => {
  const a = record({ seq: 1, time: '2026-11-14T09:30:01.000000Z' });
  const b = record({ seq: 2, time: '2026-11-14T09:30:02.000000Z' });
  const c = record({ seq: 3, time: '2026-11-14T09:30:03.000000Z' });
  const both = merge([a, b], [b, c]);
  assert.deepEqual(both.records.map(keyOf), ['b1:1', 'b1:2', 'b1:3']);
  assert.equal(both.added, 1);
  // The same number in another boot is another record.
  const other = record({ boot: 'b0', seq: 2, time: '2026-11-13T09:30:02.000000Z' });
  const across = merge([a, b], [other]);
  assert.deepEqual(across.records.map(keyOf), ['b0:2', 'b1:1', 'b1:2']);
  assert.equal(across.added, 1);
  // A record that comes twice in one answer is added once.
  assert.equal(merge([], [a, a, a]).records.length, 1);
  assert.equal(merge([], [a, a, a]).added, 1);
});

test('merge with nothing new gives the list itself, and says none were added', () => {
  const a = record({ seq: 1 });
  const old = [a];
  const same = merge(old, [{ ...a }]);
  assert.equal(same.records, old);
  assert.equal(same.added, 0);
  assert.equal(merge(old, []).records, old);
  assert.equal(merge([], []).added, 0);
});

test('merge puts older lines above and newer lines below, and orders by the microsecond, then boot, then number', () => {
  const t = (seconds, micros = 0) => `2026-11-14T09:30:${String(seconds).padStart(2, '0')}.${String(micros).padStart(6, '0')}Z`;
  const shown = [record({ seq: 5, time: t(10) }), record({ seq: 6, time: t(11) })];
  const older = [record({ seq: 2, time: t(5) }), record({ seq: 3, time: t(9, 999_999) })];
  const newer = [record({ seq: 7, time: t(11, 1) })];
  const all = merge(merge(shown, older).records, newer).records;
  assert.deepEqual(all.map((one) => one.seq), [2, 3, 5, 6, 7]);
  // The same instant: by boot, then number.
  const tied = merge([], [record({ boot: 'b2', seq: 1, time: t(1) }), record({ boot: 'b1', seq: 9, time: t(1) }), record({ boot: 'b1', seq: 4, time: t(1) })]).records;
  assert.deepEqual(tied.map(keyOf), ['b1:4', 'b1:9', 'b2:1']);
  // A page of the same second that brings nothing new is how the page knows that more than a page share it.
  assert.equal(merge(shown, shown.map((one) => ({ ...one }))).added, 0);
});

// ---- folding -----------------------------------------------------------------------------------------------------

test('lines in a row with the same source and text are one row with a count and the last time', () => {
  const same = (seq, seconds) => record({ seq, time: `2026-11-14T09:30:${seconds}.000000Z`, text: 'net1: no carrier' });
  const rows = rowsOf([same(1, '01'), same(2, '02'), same(3, '03'), record({ seq: 4, text: 'something else' }), same(5, '05')], true);
  assert.equal(rows.length, 3);
  assert.equal(rows[0].count, 3);
  assert.equal(rows[0].record.seq, 1);
  assert.equal(rows[0].last.seq, 3);
  assert.equal(rows[0].last.time, '2026-11-14T09:30:03.000000Z');
  assert.deepEqual(rows[0].records.map((one) => one.seq), [1, 2, 3]);
  assert.equal(rows[0].key, 'b1:1');
  assert.equal(rows[1].count, 1);
  // The same text again, but not in a row, starts a row of its own.
  assert.equal(rows[2].count, 1);
  assert.equal(rows[2].record.seq, 5);
});

test('lines that differ in their source or level are not folded', () => {
  const base = { text: 'same words' };
  const rows = rowsOf([
    record({ ...base, seq: 1 }), record({ ...base, seq: 2, name: 'apache' }), record({ ...base, seq: 3, name: 'apache', from: 'program' }),
    record({ ...base, seq: 4, name: 'apache', from: 'program', level: 'error' }),
  ], true);
  assert.equal(rows.length, 4);
});

test('without folding, each record is a row', () => {
  const lines = [record({ seq: 1 }), record({ seq: 2 }), record({ seq: 3 })];
  const rows = rowsOf(lines, false);
  assert.deepEqual(rows.map((row) => [row.key, row.count]), [['b1:1', 1], ['b1:2', 1], ['b1:3', 1]]);
  assert.equal(rows[0].record, lines[0]);
  assert.deepEqual(rowsOf([], true), []);
});

// ---- from a line to a view ----------------------------------------------------------------------------------------

test('only this service: a service\'s line and the kernel\'s name a source, a program\'s does not', () => {
  assert.deepEqual(sourceOf(record()), { service: 'nginx' });
  assert.deepEqual(sourceOf(record({ from: 'kernel', name: 'kernel' })), { kernel: true });
  assert.equal(sourceOf(record({ from: 'program', name: 'nukesh' })), null);
  assert.deepEqual(onlyFrom({ ...DEFAULT_QUERY, kernel: true }, record()), { ...DEFAULT_QUERY, service: 'nginx', kernel: false });
  assert.deepEqual(onlyFrom({ ...DEFAULT_QUERY, service: 'nginx' }, record({ from: 'kernel', name: 'kernel' })), { ...DEFAULT_QUERY, service: '', kernel: true });
  assert.equal(onlyFrom(DEFAULT_QUERY, record({ from: 'program', name: 'nukesh' })), null);
});

test('five minutes around a line: a custom range either side, with the source kept and the words cleared', () => {
  const line = record({ time: '2026-11-14T09:30:00.500000Z' });
  const made = aroundOf({ ...DEFAULT_QUERY, service: 'nginx', q: 'timed out', priority: 'info', zone: 'utc', fold: true, lines: 20 }, line);
  assert.equal(made.when, 'custom');
  assert.equal(made.since, utc(2026, 10, 14, 9, 25));
  assert.equal(made.until, utc(2026, 10, 14, 9, 35, 1));
  assert.equal(made.q, '');
  assert.equal(made.service, 'nginx');
  assert.equal(made.priority, 'info');
  assert.equal(made.zone, 'utc');
  assert.equal(made.fold, true);
  assert.equal(made.lines, 200);
  // The line itself is inside what the read asks for.
  const args = argsOf(made, { now: 0, zone: 'UTC' });
  assert.ok(args.since <= utc(2026, 10, 14, 9, 30) / 1000 && utc(2026, 10, 14, 9, 30) / 1000 < args.until);
  assert.deepEqual(queryOf(urlOf(made)).query, made);
});

test('the one second of a line, with the most lines a read gives', () => {
  const made = secondOf({ ...DEFAULT_QUERY, service: 'nginx', q: 'x' }, record({ time: '2026-11-14T09:41:07.987654Z' }));
  assert.equal(made.since, utc(2026, 10, 14, 9, 41, 7));
  assert.equal(made.until, utc(2026, 10, 14, 9, 41, 8));
  assert.equal(made.lines, 1000);
  assert.equal(made.service, 'nginx');
  const args = argsOf(made, { now: 0, zone: 'UTC' });
  assert.equal(args.until - args.since, 1);
  assert.equal(args.lines, 1000);
  assert.deepEqual(queryOf(urlOf(made)).query, made);
});

// ---- a saved line --------------------------------------------------------------------------------------------------------

test('a saved line is its time to the microsecond with the offset, its importance, who wrote it and the text', () => {
  const line = record();
  assert.equal(lineOf(line, PRAGUE), '2026-11-14 10:30:00.123456 +0100 warning nginx[312]: upstream timed out');
  assert.equal(lineOf(line, 'America/New_York'), '2026-11-14 04:30:00.123456 -0500 warning nginx[312]: upstream timed out');
  assert.equal(lineOf(line, 'UTC'), '2026-11-14 09:30:00.123456 +0000 warning nginx[312]: upstream timed out');
});

test('the kernel is named kernel, and a record with no process has only its name', () => {
  assert.equal(lineOf(record({ from: 'kernel', name: 'kernel', process: null, level: 'error', text: 'oops' }), 'UTC'), '2026-11-14 09:30:00.123456 +0000 error kernel: oops');
  assert.equal(lineOf(record({ process: null, from: 'service', name: 'spark', level: 'info' }), 'UTC'), '2026-11-14 09:30:00.123456 +0000 info spark: upstream timed out');
  assert.equal(lineOf(record({ from: 'program', name: 'nukesh', process: 57, level: 'debug' }), 'UTC'), '2026-11-14 09:30:00.123456 +0000 debug nukesh[57]: upstream timed out');
  assert.equal(lineOf(record({ from: 'kernel', name: 'kernel', process: 0 }), 'UTC').includes('kernel[0]'), false);
});

test('a saved line is one line: a break in the text is a space, and the rest of it is kept whole', () => {
  assert.equal(lineOf(record({ text: 'a\nb\r\nc\rd' }), 'UTC').endsWith(': a b c d'), true);
  const long = 'x'.repeat(3000);
  assert.equal(lineOf(record({ text: long }), 'UTC').endsWith(`: ${long}`), true);
  assert.equal(lineOf(record({ text: '  spaced  out: yes ' }), 'UTC').endsWith(':   spaced  out: yes '), true);
  assert.equal(lineOf(record({ text: '' }), 'UTC').endsWith('nginx[312]: '), true);
});

test('a time in the hour shown twice is told by its offset in a saved line', () => {
  assert.ok(lineOf(record({ time: '2026-10-25T00:30:00.000000Z' }), PRAGUE).startsWith('2026-10-25 02:30:00.000000 +0200 '));
  assert.ok(lineOf(record({ time: '2026-10-25T01:30:00.000000Z' }), PRAGUE).startsWith('2026-10-25 02:30:00.000000 +0100 '));
});

// ---- the follow ----------------------------------------------------------------------------------------------------------------

test('the events of a follow are read: the command, a record, the lines skipped and the end', () => {
  assert.deepEqual(followEvent('command', '{"shown":"elevate log --quiet --output json --follow --lines 0 --priority warning"}'), {
    kind: 'command', shown: 'elevate log --quiet --output json --follow --lines 0 --priority warning',
  });
  assert.deepEqual(followEvent('record', JSON.stringify(record())), { kind: 'record', record: recordOf(record()) });
  assert.deepEqual(followEvent('skipped', '{"count":37}'), { kind: 'skipped', count: 37 });
  assert.deepEqual(followEvent('ended', '{"why":"locked"}'), { kind: 'ended', why: 'locked', error: '' });
  assert.deepEqual(followEvent('ended', '{"why":"failed","error":"log: logd is not running"}'), { kind: 'ended', why: 'failed', error: 'log: logd is not running' });
  assert.deepEqual(followEvent('ended', '{"why":"failed","error":5}'), { kind: 'ended', why: 'failed', error: '' });
});

test('an event that is bad is skipped, so that the ones after it still come', () => {
  for (const [type, data] of [
    ['record', 'not json'], ['record', ''], ['record', 'null'], ['record', '5'], ['record', '[]'], ['record', '{}'],
    ['record', JSON.stringify(record({ seq: 'x' }))], ['record', JSON.stringify(record({ time: 'noon' }))], ['record', '{"time":"2026-11-14T09:30:00Z"}'],
    ['command', '{}'], ['command', '{"shown":5}'], ['command', '"log"'],
    ['skipped', '{}'], ['skipped', '{"count":0}'], ['skipped', '{"count":-4}'], ['skipped', '{"count":1.5}'], ['skipped', '{"count":"9"}'],
    ['ended', '{}'], ['ended', '{"why":3}'], ['ended', 'locked'],
    ['unknown', '{"x":1}'], ['message', '{}'], [undefined, '{}'],
  ]) {
    assert.equal(followEvent(type, data), null, `${type} ${data}`);
  }
});

test('a stream with a bad event in it gives the good ones, in order', () => {
  const stream = [
    ['command', '{"shown":"elevate log"}'],
    ['record', JSON.stringify(record({ seq: 1 }))],
    ['record', '{"seq":'],
    ['record', JSON.stringify(record({ seq: 2 }))],
    ['skipped', '{"count":12}'],
    ['record', JSON.stringify(record({ seq: 'three' }))],
    ['record', JSON.stringify(record({ seq: 4 }))],
    ['ended', '{"why":"time"}'],
  ].map(([type, data]) => followEvent(type, data)).filter(Boolean);
  assert.deepEqual(stream.map((event) => event.kind), ['command', 'record', 'record', 'skipped', 'record', 'ended']);
  assert.deepEqual(stream.filter((event) => event.kind === 'record').map((event) => event.record.seq), [1, 2, 4]);
});

test('each ending of a follow has its sentence, and a failure says what log said', () => {
  assert.equal(endedWords({ why: 'locked' }), 'Following stopped: you locked the console.');
  assert.equal(endedWords({ why: 'unlock-ended' }), 'Following stopped: your unlock ended.');
  assert.equal(endedWords({ why: 'session-ended' }), 'Following stopped: this sign-in ended.');
  assert.equal(endedWords({ why: 'replaced' }), 'Following stopped: you began following in another window.');
  assert.equal(endedWords({ why: 'time' }), 'Following stopped: it had run for an hour.');
  assert.equal(endedWords({ why: 'slow' }), 'Following stopped: the lines came faster than this page could take them.');
  assert.equal(endedWords({ why: 'stopped' }), 'Following stopped: the server is stopping.');
  assert.equal(endedWords({ why: 'failed', error: '' }), 'Following stopped: the log command ended.');
  assert.equal(endedWords({ why: 'failed' }), 'Following stopped: the log command ended.');
  assert.equal(endedWords({ why: 'failed', error: ' log: logd is not running \n' }), 'Following stopped: the log command ended. It said: log: logd is not running');
  assert.equal(endedWords({ why: 'meteor' }), 'Following stopped.');
  assert.equal(BROKEN_WORDS, 'Following stopped: the connection to the server was lost.');
});

// ---- saving and counting ----------------------------------------------------------------------------------------------------------------

test('a saved file is named for the server, the day and the minute in the zone shown', () => {
  const now = utc(2026, 10, 14, 9, 5, 59);
  assert.equal(saveName('duke-nas', now, PRAGUE), 'log-duke-nas-20261114-1005.txt');
  assert.equal(saveName('duke-nas', now, 'UTC'), 'log-duke-nas-20261114-0905.txt');
  assert.equal(saveName('duke-nas', utc(2026, 10, 14, 23, 59), PRAGUE), 'log-duke-nas-20261115-0059.txt');
  assert.equal(saveName('duke-nas', utc(2026, 0, 2, 3, 4), 'UTC'), 'log-duke-nas-20260102-0304.txt');
});

test('a host name that is no file name is made into one', () => {
  const now = utc(2026, 10, 14, 9, 5);
  assert.equal(saveName('Duke NAS/1', now, 'UTC'), 'log-Duke-NAS-1-20261114-0905.txt');
  assert.equal(saveName('../etc', now, 'UTC'), 'log-etc-20261114-0905.txt');
  assert.equal(saveName('', now, 'UTC'), 'log-server-20261114-0905.txt');
  assert.equal(saveName('host.example.org', now, 'UTC'), 'log-host.example.org-20261114-0905.txt');
});

test('new lines are counted in words', () => {
  assert.equal(newLinesWords(1), '1 new line');
  assert.equal(newLinesWords(2), '2 new lines');
  assert.equal(newLinesWords(12), '12 new lines');
});

test('a screen reader is told how many lines came, at most once in five seconds, and never when none came', (context) => {
  context.mock.timers.enable({ apis: ['setTimeout', 'Date'], now: 1_000_000 });
  const said = [];
  const announcer = makeAnnouncer((words) => said.push([Date.now(), words]));
  // The first lines are told at once.
  announcer.add(3);
  assert.deepEqual(said, [[1_000_000, '3 new lines']]);
  // Lines that come inside the five seconds wait, and are counted together.
  context.mock.timers.tick(1000);
  announcer.add(1);
  context.mock.timers.tick(1000);
  announcer.add(4);
  assert.equal(said.length, 1);
  context.mock.timers.tick(2999);
  assert.equal(said.length, 1);
  context.mock.timers.tick(1);
  assert.deepEqual(said[1], [1_005_000, '5 new lines']);
  // Nothing came for a while: nothing is said.
  context.mock.timers.tick(60_000);
  assert.equal(said.length, 2);
  // A line after a quiet spell is told at once.
  announcer.add(1);
  assert.deepEqual(said[2], [1_065_000, '1 new line']);
  announcer.add(0);
  announcer.add(-2);
  announcer.add(Number.NaN);
  context.mock.timers.tick(10_000);
  assert.equal(said.length, 3);
});

test('stopping the announcer drops what waits', (context) => {
  context.mock.timers.enable({ apis: ['setTimeout', 'Date'], now: 0 });
  const said = [];
  const announcer = makeAnnouncer((words) => said.push(words));
  announcer.add(1);
  announcer.add(7);
  assert.deepEqual(said, ['1 new line']);
  announcer.stop();
  context.mock.timers.tick(20_000);
  assert.deepEqual(said, ['1 new line']);
  // It can be used again after it stops.
  announcer.add(2);
  assert.deepEqual(said, ['1 new line', '2 new lines']);
  announcer.stop();
  announcer.stop();
});

test('what a page says of the lines a person reads', () => {
  assert.equal(whoWords(true, true), null);
  assert.deepEqual(whoWords(true, false), { words: 'You see the lines of your own programs. Unlock to read the whole log.', unlock: true });
  assert.deepEqual(whoWords(false, false), { words: 'You see the lines of your own programs. The machine\'s log is for administrators.', unlock: false });
});
