// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The dev server of devserver.mjs with the answers the Storage page reads,
// from storage-fixtures.mjs, for looking at the page in a browser (design
// 17: each board checked in dark and light, at 1280 and 390 wide). Never
// installed. The fixtures' "now" is `NOW` of storage-fixtures.mjs: a browser
// that looks at the page sets its clock there.
//
//   node userspace/admin/tests/storage-server.mjs [port]
//
// `state.storage.answer` is what `GET /api/1/storage` gives, and a test
// changes it and reloads; `calls` lists the writes the page made.

import { freshState, startServer } from './devserver.mjs';
import {
  GB, NOW, series, showWearing, usage30d,
} from './storage-fixtures.mjs';

/** The log's own answer, as `logtrim --show --json` gives it. */
const LOG = Object.freeze({
  usage: { bytes: 1_288_490_188, files: 92, audit_bytes: 83_886_080, directory: '/system/state/log' },
  limits: { keep_days: 90, max_bytes: 4_294_967_296, max_percent: 10, audit_keep_days: 365 },
  cap: 4_294_967_296,
  filesystem_size: 255_999_999_999,
  wrong: [],
});

/** What `diskhealth --json sdb` gives for a disk that is wearing. */
const CHECKED = Object.freeze([{
  disk: 'sdb',
  hours: 21804,
  attributes: [
    { id: 5, name: 'Reallocated_Sector_Ct', prefail: true, value: 100, worst: 100, threshold: 10, raw: 8 },
    { id: 9, name: 'Power_On_Hours', prefail: false, value: 90, worst: 90, threshold: 0, raw: 21804 },
  ],
  self_tests: [{ kind: 'extended', result: 'passed', at_hours: 21500 }],
}]);

/** The reads and writes of the last hour, a point a minute. */
function lastHour() {
  const values = (peak) => Array.from({ length: 61 }, (_, at) => (at === 24 ? peak : 20_000 + (at % 7) * 3000));
  return [
    { series: 'disk.read{sda}', points: series(NOW - 3600, 60, values(2_100_000)) },
    { series: 'disk.write{sda}', points: series(NOW - 3600, 60, values(1_400_000)) },
  ];
}

/** The actions the page runs, as `GET /api/1/actions` lists them. */
const ACTIONS = [
  { id: 'storage.health', title: 'Check a disk now', area: 'storage', changes: false, method: 'GET', path: '/api/1/storage/disks/{disk}/health', runs: 'as-administrator', long: false },
  { id: 'storage.scrub', title: 'Scrub a volume', area: 'storage', changes: true, method: 'POST', path: '/api/1/storage/scrub', runs: 'as-administrator', long: true },
  { id: 'storage.grow', title: 'Grow a disk into its free space', area: 'storage', changes: true, method: 'POST', path: '/api/1/storage/disks/{disk}/grow', runs: 'as-administrator', long: false },
  { id: 'storage.log', title: 'Read the log\'s use and limits', area: 'storage', changes: false, method: 'GET', path: '/api/1/storage/log', runs: 'as-administrator', long: false },
  { id: 'storage.log-set', title: 'Change the log\'s limits', area: 'storage', changes: true, method: 'POST', path: '/api/1/storage/log/limits', runs: 'as-administrator', long: false },
  { id: 'storage.log-trim', title: 'Trim the log now', area: 'storage', changes: true, method: 'POST', path: '/api/1/storage/log/trim', runs: 'as-administrator', long: false },
];

/** The state of the fake API: devserver's, with what the Storage page reads. */
export function storageState() {
  const state = freshState();
  state.actions.push(...ACTIONS.map((action) => ({ ...action })));
  state.tasks = [];
  state.storage = {
    answer: showWearing(),
    usage30: usage30d(),
    log: structuredClone(LOG),
    checked: structuredClone(CHECKED),
    // The seconds a scrub runs for; 0 ends it as it starts.
    scrubMs: 1500,
    calls: [],
  };
  return state;
}

/** Starts the server; gives what `startServer` gives. */
export async function startStorage({ port = 0, state = storageState() } = {}) {
  const started = await startServer({ port, state });
  const { server } = started;
  const original = server.listeners('request');
  server.removeAllListeners('request');

  const json = (response, status, body) => {
    response.writeHead(status, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
    response.end(JSON.stringify(body));
    return true;
  };
  const error = (response, status, id, title, detail = '') => json(response, status, { error: { id, title, detail } });
  const locked = (response) => error(response, 403, 'ADM-106', 'The console is locked', 'Unlock it to make changes.');

  /** Answers the routes of the Storage page, and gives true when it did. */
  async function mine(request, response) {
    const { pathname } = new URL(request.url, 'http://localhost');
    if (!pathname.startsWith('/api/1/storage') || !state.signedIn || state.offline) return false;
    const data = state.storage;
    const get = request.method === 'GET';
    let body = '';
    for await (const chunk of request) body += chunk;
    const args = body ? JSON.parse(body) : {};
    if (get && pathname === '/api/1/storage') return json(response, 200, data.answer);
    const range = /^\/api\/1\/storage\/usage\/([^/]+)$/.exec(pathname);
    if (get && range) {
      if (range[1] === '1h') return json(response, 200, lastHour());
      if (range[1] === '30d') return data.usage30 === 'none' ? error(response, 422, 'ADM-123', 'The command did not work', 'metrics: no points between now and then.') : json(response, 200, data.usage30);
      return error(response, 400, 'ADM-125', 'The usage range is 1h or 30d');
    }
    // What needs an administrator who has unlocked the console.
    if (!state.session.admin) return error(response, 403, 'ADM-105', 'Only administrators can do that', 'This account may look and not change.');
    if (!state.session.unlock_seconds) return locked(response);
    const health = /^\/api\/1\/storage\/disks\/([^/]+)\/health$/.exec(pathname);
    if (get && health) return json(response, 200, data.checked);
    if (get && pathname === '/api/1/storage/log') return json(response, 200, data.log);
    data.calls.push({ path: pathname, args });
    if (pathname === '/api/1/storage/scrub') {
      const id = Math.max(1900, ...state.tasks.map((one) => one.id)) + 1;
      const task = {
        id, action: 'storage.scrub', title: 'Scrub a volume', args, who: state.session.name, how: 'console', started: NOW, state: 'running', step: 0, steps: 0,
        words: 'scrub: 41%', percent: 41, ended: null, status: null, note: null, changed: null,
      };
      state.tasks.unshift(task);
      // An event holds the task and not its arguments, as the shell reads them.
      const event = () => started.send('task', Object.fromEntries(Object.entries(task).filter(([key]) => key !== 'args')));
      event();
      setTimeout(() => {
        Object.assign(task, { state: 'done', percent: 100, status: 0, ended: NOW + 2, changed: true });
        event();
      }, data.scrubMs);
      return json(response, 202, { task: id });
    }
    if (/^\/api\/1\/storage\/disks\/[^/]+\/grow$/.test(pathname)) return json(response, 200, { done: true, changed: true, output: [] });
    if (pathname === '/api/1/storage/log/limits') {
      for (const [key, value] of Object.entries(args)) {
        if (key === 'max_size') data.log.limits.max_bytes = value === '0' ? 0 : data.log.limits.max_bytes;
        else data.log.limits[key] = value;
      }
      return json(response, 200, { done: true, changed: true, output: [] });
    }
    if (pathname === '/api/1/storage/log/trim') {
      data.log.usage.bytes = Math.floor(data.log.usage.bytes / 4);
      data.log.usage.files = 24;
      return json(response, 200, { done: true, changed: true, output: [`trimmed to ${GB / 1000} MB`] });
    }
    return false;
  }

  server.on('request', (request, response) => {
    mine(request, response).then((answered) => {
      if (!answered) for (const listener of original) listener.call(server, request, response);
    }, () => {
      response.writeHead(500);
      response.end();
    });
  });
  return started;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const { port } = await startStorage({ port: Number(process.argv[2] ?? 8099) });
  console.log(`The console's pages are at http://127.0.0.1:${port}/`);
}
