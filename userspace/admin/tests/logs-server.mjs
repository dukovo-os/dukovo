// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Overview's fake server with the answers the Logs page reads, for
// looking at the page in a browser (design 17: dark and light, at 1280 and
// 390 wide) and for the Node tests of its client. Never installed. The lines
// are made from the time the server starts, so a browser needs no clock of
// its own.
//
//   node userspace/admin/tests/logs-server.mjs [port]
//
// `state.logs` holds what the routes give; a test changes it and reloads:
// `own` is the lines the reader sees, `refuse` is the answer of a follow
// that is refused before it starts ('busy' or null), `failNext` is an error
// body ({status, id, title, detail, field}) for the next read, and `push`,
// `endFollow` and `followers` act on the streams that are open.

import { startOverview } from './overview-server.mjs';
import { NOW } from './overview-fixtures.mjs';

const LEVELS = ['error', 'warning', 'info', 'debug'];
const NAMES = new Set(['lines', 'service', 'kernel', 'priority', 'since', 'until', 'grep']);
const FOLLOW_NAMES = new Set([...NAMES].filter((name) => !['lines', 'since', 'until'].includes(name)).concat('all'));
const BOOT = '5f8b0c2e9a7d4c11b3a96f2d0e1c7a44';

// The Usage ranges, and how long a point of `metrics.around` is for each.
const USAGE = { '1h': { span: 3600, every: 60 }, '24h': { span: 86400, every: 900 }, '7d': { span: 7 * 86400, every: 7200 }, '1y': { span: 365 * 86400, every: 86400 } };
const AROUND_EVERY = { '1h': 60, '24h': 60, '7d': 900, '1y': 3600 };

/** `GET /api/1/metrics/usage/{range}` for the clock `now` (Unix seconds): the seven series, with a spike of the CPU and the disk at four tenths of the way. */
export function usageAt(range, now) {
  const { span, every } = USAGE[range];
  const end = Math.floor(now / every) * every;
  const count = Math.floor(span / every);
  const at = (index) => end - (count - index) * every;
  const series = (name, value) => ({ series: name, points: Array.from({ length: count + 1 }, (_, index) => [at(index), value(index, count)]) });
  const spike = (index, last, high, low) => (index === Math.floor(last * 0.4) ? high : low);
  return [
    series('cpu', (index, last) => spike(index, last, 92, 8 + (index % 7))),
    series('memory.used', (index) => 7_200_000_000 + (index % 5) * 40_000_000),
    series('memory.available', (index) => 8_800_000_000 - (index % 5) * 40_000_000),
    series('disk.read{vda}', (index, last) => spike(index, last, 2_100_000, 20_000)),
    series('disk.write{vda}', (index, last) => spike(index, last, 1_400_000, 12_000)),
    series('net.rx{eth0}', (index) => 200_000 + (index % 11) * 150_000),
    series('net.tx{eth0}', (index) => 60_000 + (index % 13) * 30_000),
  ];
}

/**
 * `GET /api/1/metrics/around/{at}?range=R`: the service series, 60 points
 * before the history's step that holds `at` and 10 from it, with postgres at
 * 1 percent before the step and 41 from it, and php-fpm moving from 50 to 60.
 */
export function aroundAt(at, range) {
  const every = AROUND_EVERY[range];
  // The points are on the history's grid, whatever time is asked for.
  const start = Math.floor(at / every) * every;
  const times = Array.from({ length: 71 }, (_, index) => start - 60 * every + index * every);
  const series = (name, value) => ({ series: name, points: times.map((time) => [time, value(time)]) });
  const after = (before, during) => (time) => (time < start ? before : during);
  return [
    series('service.cpu{postgres}', after(1, 41)),
    series('service.cpu{php-fpm}', after(50, 60)),
    series('service.cpu{nginx}', after(12, 12)),
    series('service.cpu{valkey}', after(0.2, 0.3)),
    series('service.memory{postgres}', after(1_900_000_000, 2_400_000_000)),
    series('service.memory{php-fpm}', after(170_000_000, 170_000_000)),
    series('service.memory{nginx}', after(190_000_000, 190_000_000)),
  ];
}

/** The decimal of a time with six digits: "2026-11-14T10:12:03.123456Z". */
export function stampOf(ms, micros = 0) {
  return `${new Date(ms).toISOString().slice(0, 19)}.${String(((ms % 1000) * 1000 + micros + 1_000_000) % 1_000_000).padStart(6, '0')}Z`;
}

// What the lines say, as (from, name, level, text, user).
const SAYINGS = [
  ['kernel', 'kernel', 'info', 'usb 1-2: new high-speed USB device number 4 using xhci_hcd'],
  ['kernel', 'kernel', 'warning', 'nf_conntrack: table full, dropping packet'],
  ['kernel', 'kernel', 'error', 'EXT4-fs error (device sda2): ext4_find_entry:1455: inode #131080: comm nginx: reading directory lblock 0'],
  ['service', 'nginx', 'info', 'GET /photos/2026/IMG_0412.jpg 200 1843211'],
  ['service', 'nginx', 'info', 'GET /photos/2026/IMG_0413.jpg 200 1633091'],
  ['service', 'nginx', 'warning', 'upstream timed out (110: Connection timed out) while reading response header from upstream'],
  ['service', 'postgres', 'info', 'checkpoint complete: wrote 412 buffers (2.5%)'],
  ['service', 'postgres', 'warning', 'could not receive data from client: Connection reset by peer'],
  ['service', 'php-fpm', 'debug', 'pool www: child 4127 started'],
  ['service', 'backup-sync', 'error', 'the NAS at 192.168.1.20 did not answer'],
  ['service', 'valkey', 'info', 'Background saving terminated with success'],
  ['program', 'elevate', 'info', 'alice ran: service nginx reload', 1000],
  ['service', 'admind', 'info', 'sign-in of alice from 192.168.1.40', 1000],
];

/** About `count` lines, the newest a few seconds before `now`, in the order the log has them. */
export function logRecords(now, count = 900) {
  let seed = 7;
  const next = () => {
    seed = (seed * 1_103_515_245 + 12_345) % 2_147_483_648;
    return seed / 2_147_483_648;
  };
  const records = [];
  let at = now - 4000;
  for (let seq = count; seq >= 1; seq -= 1) {
    const saying = SAYINGS[Math.floor(next() * SAYINGS.length)];
    // A line that repeats shows what folding does.
    const [from, name, level, text, user] = saying;
    records.push({
      time: stampOf(at, Math.floor(next() * 1000)),
      boot: BOOT,
      seq,
      level,
      from,
      name,
      process: from === 'kernel' ? null : 1000 + Math.floor(next() * 400),
      user: user ?? null,
      audit: name === 'elevate' || name === 'admind',
      text,
    });
    at -= Math.floor(next() * (next() < 0.1 ? 90_000 : 9000)) + 200;
  }
  return records.reverse();
}

/** Starts the server; gives what `startOverview` gives, with `logs` to act on the streams. */
export async function startLogs({ port = 0 } = {}) {
  const started = await startOverview({ port });
  const { server, state } = started;
  const now = Date.now();
  const logs = {
    records: logRecords(now),
    // The lines a reader (an account that is not an administrator) sees: the ones about their own account.
    own: (record) => record.user === 1000,
    refuse: null,
    followers: new Set(),
    seen: [],
    nextSeq: 10_000,
    nextCommand: null,
    failNext: null,
    aroundFails: false,
    // Rows of the journal that a test adds, with `at` in Unix seconds.
    changes: [],
  };
  state.logs = logs;
  const original = server.listeners('request');
  server.removeAllListeners('request');

  const json = (response, status, body, headers = {}) => {
    response.writeHead(status, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store', ...headers });
    response.end(JSON.stringify(body));
    return true;
  };
  const error = (response, status, id, title, detail = '', more = {}, headers = {}) => json(response, status, { error: { id, title, detail, ...more } }, headers);

  /** What a read or a follow is not allowed to be; gives true after it answered. */
  function refused(response, url, all, names) {
    if (all && !state.session.admin) {
      return error(response, 403, 'ADM-122', 'This account can look but not change', 'Reading the whole log is for administrators.');
    }
    if (all && !state.session.unlock_seconds) return error(response, 403, 'ADM-106', 'The console is locked', 'Unlock it to read the whole log.');
    for (const name of url.searchParams.keys()) {
      if (!names.has(name)) return error(response, 400, 'ADM-108', 'The request is not right', `The log does not take "${name}".`);
    }
    const priority = url.searchParams.get('priority');
    if (priority !== null && !LEVELS.includes(priority)) {
      return error(response, 400, 'ADM-125', 'That importance is not known', 'It is error, warning, info or debug.', { field: 'priority' });
    }
    const grep = url.searchParams.get('grep');
    if (grep !== null) {
      // eslint-disable-next-line no-control-regex
      if (grep.length < 1 || grep.length > 200 || /[\u0000-\u001f\u007f]/.test(grep)) {
        return error(response, 400, 'ADM-125', 'The words to look for are 1 to 200 characters', '', { field: 'grep' });
      }
      try {
        new RegExp(grep);
      } catch {
        return error(response, 400, 'ADM-125', 'That is not a pattern the log can read', `"${grep}" is not a regular expression.`, { field: 'grep' });
      }
    }
    return false;
  }

  /** The lines a read of `url` gives, oldest first. */
  function select(url, all) {
    const get = (name) => url.searchParams.get(name);
    const since = get('since') === null ? -Infinity : Number(get('since'));
    const until = get('until') === null ? Infinity : Number(get('until'));
    const lines = get('lines') === null ? 200 : Number(get('lines'));
    const level = LEVELS.indexOf(get('priority') ?? 'debug');
    const grep = get('grep');
    const matcher = grep === null ? null : new RegExp(grep, /[A-Z]/.test(grep) ? '' : 'i');
    const found = logs.records.filter((record) => {
      const seconds = Date.parse(`${record.time.slice(0, 19)}Z`) / 1000;
      if (seconds < since || seconds >= until) return false;
      if (!all && !logs.own(record)) return false;
      if (LEVELS.indexOf(record.level) > level) return false;
      if (get('service') !== null && !(record.from === 'service' && record.name === get('service'))) return false;
      if (get('kernel') === 'true' && record.from !== 'kernel') return false;
      return !matcher || matcher.test(record.text);
    });
    return found.slice(-lines);
  }

  /** A time in whole seconds as `log` reads it: "2026-11-14 09:30:00 UTC". */
  const utcWords = (seconds) => `'${new Date(Number(seconds) * 1000).toISOString().slice(0, 19).replace('T', ' ')} UTC'`;

  /** The command a read shows, as `log` is run: the times in UTC, as the server writes them. */
  const shown = (url, all) => {
    const words = ['log'];
    if (!all) words.push('--own');
    for (const [name, value] of url.searchParams) {
      if (name === 'kernel') words.push('--kernel');
      else words.push(`--${name}`, name === 'since' || name === 'until' ? utcWords(value) : value);
    }
    return words.join(' ');
  };

  function follow(request, response, url) {
    const all = url.searchParams.get('all') === 'true';
    if (logs.refuse === 'busy') {
      return error(response, 429, 'ADM-134', 'Too many logs are being followed', 'Eight follows are open.', { fix: 'Wait a moment and try again.' }, { 'Retry-After': '30' });
    }
    if (refused(response, url, all, FOLLOW_NAMES)) return true;
    // One follow for each session: the one that was going is told that another began.
    for (const old of [...logs.followers]) {
      old.response.write(`event: ended\ndata: ${JSON.stringify({ why: 'replaced' })}\n\n`);
      old.response.end();
      logs.followers.delete(old);
    }
    response.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-store' });
    const stream = { response, url, all };
    logs.followers.add(stream);
    logs.seen.push(request.url);
    response.write(': open\n\n');
    const words = ['log', '--follow', '--quiet'];
    if (!all) words.push('--own');
    for (const [name, value] of url.searchParams) if (name !== 'all') words.push(`--${name}`, value);
    response.write(`event: command\ndata: ${JSON.stringify({ shown: logs.nextCommand ?? words.join(' ') })}\n\n`);
    request.on('close', () => logs.followers.delete(stream));
    return true;
  }

  /** Gives each open follow a line that it asks for. */
  logs.push = (record) => {
    const full = {
      time: stampOf(Date.now()), boot: BOOT, seq: logs.nextSeq += 1, level: 'info', from: 'service', name: 'nginx', process: 1234, user: null, audit: false, text: 'a new line', ...record,
    };
    logs.records.push(full);
    for (const { response, url, all } of logs.followers) {
      if (!all && !logs.own(full)) continue;
      if (LEVELS.indexOf(full.level) > LEVELS.indexOf(url.searchParams.get('priority') ?? 'debug')) continue;
      if (url.searchParams.get('service') && url.searchParams.get('service') !== full.name) continue;
      if (url.searchParams.get('kernel') === 'true' && full.from !== 'kernel') continue;
      const grep = url.searchParams.get('grep');
      if (grep && !new RegExp(grep, /[A-Z]/.test(grep) ? '' : 'i').test(full.text)) continue;
      response.write(`event: record\ndata: ${JSON.stringify(full)}\n\n`);
    }
    return full;
  };
  /** Ends every open follow with `why` (and `error` for 'failed'). */
  logs.endFollow = (why, said) => {
    for (const stream of logs.followers) {
      stream.response.write(`event: ended\ndata: ${JSON.stringify(said === undefined ? { why } : { why, error: said })}\n\n`);
      stream.response.end();
    }
    logs.followers.clear();
  };
  /** Cuts every open follow with no word, as a lost connection does. */
  logs.cut = () => {
    for (const stream of logs.followers) stream.response.destroy();
    logs.followers.clear();
  };

  function mine(request, response) {
    const url = new URL(request.url, 'http://localhost');
    const { pathname } = url;
    if (!pathname.startsWith('/api/1/') || !state.signedIn || state.offline) return false;
    if (request.method === 'GET' && (pathname === '/api/1/logs' || pathname === '/api/1/logs/all')) {
      const all = pathname.endsWith('/all');
      logs.seen.push(request.url);
      if (logs.failNext) {
        const { status, id, title, detail, ...more } = logs.failNext;
        logs.failNext = null;
        return error(response, status, id, title, detail, more);
      }
      if (refused(response, url, all, NAMES)) return true;
      const since = url.searchParams.get('since');
      const until = url.searchParams.get('until');
      for (const [name, value] of [['since', since], ['until', until]]) {
        if (value !== null && !/^\d+$/.test(value)) return error(response, 400, 'ADM-125', 'That time is not a whole number of seconds', '', { field: name });
      }
      if (since !== null && until !== null && Number(until) < Number(since)) {
        return error(response, 400, 'ADM-125', 'The end is before the start', '', { field: 'until' });
      }
      const lines = url.searchParams.get('lines');
      if (lines !== null && !(/^\d+$/.test(lines) && Number(lines) >= 1 && Number(lines) <= 1000)) {
        return error(response, 400, 'ADM-125', 'The number of lines is 1 to 1000', '', { field: 'lines' });
      }
      return json(response, 200, select(url, all));
    }
    if (request.method === 'GET' && pathname === '/api/1/logs/follow') return follow(request, response, url);
    const range = /^\/api\/1\/metrics\/usage\/([^/]+)$/.exec(pathname);
    if (request.method === 'GET' && range && USAGE[range[1]] && state.overview.usage !== 'none') return json(response, 200, usageAt(range[1], Date.now() / 1000));
    const around = /^\/api\/1\/metrics\/around\/([^/]+)$/.exec(pathname);
    if (request.method === 'GET' && around) {
      logs.seen.push(request.url);
      const word = url.searchParams.get('range');
      if (!/^\d+$/.test(around[1]) || Number(around[1]) > Date.now() / 1000 + 60) return error(response, 400, 'ADM-125', 'That moment is not one the history can show', '', { field: 'at' });
      if (!AROUND_EVERY[word]) return error(response, 400, 'ADM-125', 'The range is 1h, 24h, 7d or 1y', '', { field: 'range' });
      if (logs.aroundFails) return error(response, 500, 'ADM-121', 'The console could not do that', 'metrics: it failed.');
      return json(response, 200, aroundAt(Number(around[1]), word));
    }
    // The journal as it was at the fixtures' clock, moved to this one.
    if (request.method === 'GET' && pathname === '/api/1/changes') {
      const shift = Math.floor(Date.now() / 1000) - NOW;
      const since = Number(url.searchParams.get('since') ?? 0);
      const limit = Number(url.searchParams.get('limit') ?? 50);
      const rows = state.overview.changes.changes.map((change) => ({ ...change, at: change.at + shift })).concat(logs.changes);
      return json(response, 200, { changes: rows.filter((change) => change.at >= since).sort((a, b) => b.at - a.at).slice(0, limit) });
    }
    // Locking ends the follows of the whole log, as admind does.
    if (request.method === 'POST' && pathname === '/api/1/lock') {
      for (const stream of [...logs.followers]) {
        if (!stream.all) continue;
        stream.response.write(`event: ended\ndata: ${JSON.stringify({ why: 'locked' })}\n\n`);
        stream.response.end();
        logs.followers.delete(stream);
      }
    }
    if (request.method === 'POST' && pathname === '/api/1/show') {
      let body = '';
      request.on('data', (piece) => { body += piece; });
      request.on('end', () => {
        const wanted = JSON.parse(body || '{}');
        const args = new URL('http://localhost/');
        for (const [name, value] of Object.entries(wanted.args ?? {})) args.searchParams.set(name, String(value));
        const all = wanted.action === 'logs.read-all';
        json(response, 200, {
          command: shown(args, all), api: `curl https://duke-nas:9090/api/1/logs${all ? '/all' : ''}?${args.searchParams}`, ansible: null, files: [],
        });
      });
      return true;
    }
    return false;
  }

  server.on('request', (request, response) => {
    if (mine(request, response)) return;
    for (const listener of original) listener.call(server, request, response);
  });
  return { ...started, logs };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const { port } = await startLogs({ port: Number(process.argv[2] ?? 8099) });
  console.log(`The console's pages are at http://127.0.0.1:${port}/`);
}
