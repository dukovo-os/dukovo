// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The dev server of devserver.mjs with the answers the Overview, /welcome and
// /setup read, from overview-fixtures.mjs, for looking at those pages in a
// browser (design 17: each board checked in dark and light, at 1280 and 390
// wide). Never installed. The fixtures' "now" is Nov 14 2026, 10:12 UTC: a
// browser that looks at the pages sets its clock there.
//
//   node userspace/admin/tests/overview-server.mjs [port]
//
// `state.overview` holds what each route gives, and a test changes it and
// reloads: `checks` is the kept result, 'missing' (ADM-129) or 'none'
// (ADM-123); `usage` is `'none'` for a machine that has no history yet;
// `runMs` is how long the lab's long action runs.

import { freshState, startServer } from './devserver.mjs';
import {
  about, certificates, change12, changes, checks, metricsNow, notices, secondStep, services, tasks, usage,
} from './overview-fixtures.mjs';

const RANGES = { '1h': { span: 3600, every: 60 }, '24h': { span: 86400, every: 900 }, '7d': { span: 7 * 86400, every: 7200 }, '1y': { span: 365 * 86400, every: 86400 } };

/** The state of the fake API: devserver's, with what the Overview reads. */
export function overviewState() {
  const state = freshState();
  state.prefs.zone = 'Europe/Prague';
  state.actions.push({ id: 'lab.long', title: 'Run the lab\'s long test', area: 'services', changes: true, method: 'POST', path: '/api/1/lab/long', runs: 'as-administrator', long: true });
  state.tasks = tasks.tasks.map((task) => ({ ...task }));
  state.overview = {
    checks: structuredClone(checks),
    about: structuredClone(about),
    notices: structuredClone(notices),
    services: structuredClone(services),
    certificates: structuredClone(certificates),
    changes: structuredClone(changes),
    change12: structuredClone(change12),
    metricsNow: structuredClone(metricsNow),
    usage: 'some',
    secondStep: structuredClone(secondStep),
    kept: [],
    // A change's own answer where it has more than the list says (the diff of a hand edit a test adds), by number.
    details: {},
    runMs: 800,
  };
  return state;
}

/** Starts the server; gives what `startServer` gives. */
export async function startOverview({ port = 0, state = overviewState() } = {}) {
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

  /** Answers the routes of the Overview, and gives true when it did. */
  function mine(request, response) {
    const url = new URL(request.url, 'http://localhost');
    const { pathname } = url;
    if (!pathname.startsWith('/api/1/') || !state.signedIn || state.offline) return false;
    const data = state.overview;
    const get = request.method === 'GET';
    if (get && pathname === '/api/1/checks/last') {
      if (data.checks === 'missing') return error(response, 501, 'ADM-129', 'This server does not have that command yet', 'The command `dukovo-check` is not on this machine.');
      if (data.checks === 'none' || !state.session.admin) return error(response, 422, 'ADM-123', 'The command did not work', 'dukovo-check: no result is kept yet.');
      return json(response, 200, data.checks);
    }
    if (get && pathname === '/api/1/console/about') return json(response, 200, data.about);
    if (get && pathname === '/api/1/console/notices') return json(response, 200, data.notices);
    if (get && pathname === '/api/1/metrics/now') return json(response, 200, data.metricsNow);
    const range = /^\/api\/1\/metrics\/usage\/([^/]+)$/.exec(pathname);
    if (get && range) {
      if (!RANGES[range[1]]) return error(response, 400, 'ADM-125', 'The usage range is 1h, 24h, 7d or 1y');
      if (data.usage === 'none') return error(response, 422, 'ADM-123', 'The command did not work', 'metrics: no points of cpu between now and then.');
      return json(response, 200, usage(RANGES[range[1]]));
    }
    if (get && pathname === '/api/1/services') return json(response, 200, data.services);
    if (get && pathname === '/api/1/certificates') return json(response, 200, data.certificates);
    if (get && pathname === '/api/1/tasks') return json(response, 200, { tasks: state.tasks });
    if (get && pathname === '/api/1/changes') {
      const since = Number(url.searchParams.get('since') ?? 0);
      const limit = Number(url.searchParams.get('limit') ?? 50);
      return json(response, 200, { changes: data.changes.changes.filter((change) => change.at >= since).slice(0, limit) });
    }
    const one = /^\/api\/1\/changes\/(\d+)$/.exec(pathname);
    if (get && one) {
      const found = data.changes.changes.find((change) => change.id === Number(one[1]));
      if (!found) return error(response, 404, 'ADM-127', 'No such change');
      if (found.id === 12) return json(response, 200, data.change12);
      if (data.details[found.id]) return json(response, 200, data.details[found.id]);
      return json(response, 200, { ...found, put_back: { possible: found.files.length > 0, command: `elevate admin put-back ${found.id}` } });
    }
    const keep = /^\/api\/1\/changes\/(\d+)\/keep$/.exec(pathname);
    if (request.method === 'POST' && keep) {
      if (!state.session.unlock_seconds) return error(response, 403, 'ADM-106', 'The console is locked', 'Unlock it to keep a change.');
      const found = data.changes.changes.find((change) => change.id === Number(keep[1]));
      found.seen = { by: state.session.name, at: Math.floor(Date.now() / 1000) };
      if (data.details[found.id]) data.details[found.id].seen = found.seen;
      data.kept.push(found.id);
      return json(response, 200, found);
    }
    const putBack = /^\/api\/1\/changes\/(\d+)\/put-back$/.exec(pathname);
    if (request.method === 'POST' && putBack) {
      if (!state.session.unlock_seconds) return error(response, 403, 'ADM-106', 'The console is locked', 'Unlock it to put a change back.');
      // As the journal does: the change stays, and the put back is a row of its own that names it.
      const was = data.details[putBack[1]] ?? data.changes.changes.find((change) => change.id === Number(putBack[1]));
      const id = Math.max(...data.changes.changes.map((change) => change.id)) + 1;
      const row = {
        id, at: Math.floor(Date.now() / 1000), who: state.session.name, how: 'console', action: 'changes.put-back', title: 'Put a change back',
        argv: null, status: 0, task: null, note: `puts back change ${putBack[1]}`, seen: null,
        files: (was?.files ?? []).map((file) => ({ ...file, diff: typeof file.diff === 'string' ? file.diff.split('\n').map((line) => (line.startsWith('+') && !line.startsWith('+++') ? `-${line.slice(1)}` : line.startsWith('-') && !line.startsWith('---') ? `+${line.slice(1)}` : line)).join('\n') : file.diff })),
      };
      data.changes.changes.unshift(row);
      data.details[id] = { ...row, put_back: { possible: false, command: null } };
      return json(response, 200, { done: true, output: [], command: `elevate admin put-back ${putBack[1]}`, change: id });
    }
    if (get && pathname === '/api/1/account/second-step') return json(response, 200, data.secondStep);
    // The lab's long action: a task that ends in a moment, so that /setup's step is done after it.
    if (request.method === 'POST' && pathname === '/api/1/lab/long') {
      if (!state.session.unlock_seconds) return error(response, 403, 'ADM-106', 'The console is locked', 'Unlock it to run this.');
      const id = Math.max(...state.tasks.map((one) => one.id)) + 1;
      const ran = { ...state.tasks[0], id, action: 'lab.long', title: 'Run the lab\'s long test', state: 'running', ended: null, status: null, note: null, args: { seconds: 3 } };
      state.tasks.unshift(ran);
      started.send('task', { id, state: 'running' });
      setTimeout(() => {
        Object.assign(ran, { state: 'done', status: 0, ended: Math.floor(Date.now() / 1000) });
        started.send('task', { id, state: 'done' });
      }, data.runMs);
      return json(response, 202, { task: id });
    }
    return false;
  }

  server.on('request', (request, response) => {
    if (mine(request, response)) return;
    for (const listener of original) listener.call(server, request, response);
  });
  return started;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const { port } = await startOverview({ port: Number(process.argv[2] ?? 8099) });
  console.log(`The console's pages are at http://127.0.0.1:${port}/`);
}
