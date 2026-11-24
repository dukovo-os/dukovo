// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A small static server and a fake API for looking at the console's pages
// in a browser (design 17: each board checked in dark and light, at 1280
// and 390 wide). Never installed: install_admin leaves tests/ out. It serves
// the console's folder and docs/errors as /help/errors, and answers the
// API's routes with a state a test can change through `server.state`.
//
//   node userspace/admin/tests/devserver.mjs [port]

import { readFileSync, readdirSync, statSync } from 'node:fs';
import { createServer } from 'node:http';
import { extname, join, normalize } from 'node:path';

const ADMIN = new URL('../', import.meta.url).pathname;
const ERRORS = new URL('../../../docs/errors/', import.meta.url).pathname;

const TYPES = {
  '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml', '.json': 'application/json', '.md': 'text/markdown',
};

/** The state the fake API answers from; a test changes it and reloads. */
export function freshState() {
  const now = Math.floor(Date.now() / 1000);
  return {
    session: {
      name: 'alice', full_name: 'Alice Martin', admin: true, host: 'duke-nas', csrf: 'csrf-token', unlock_seconds: null, needs_step: false,
      kinds: ['password', 'app'], terminal: true,
    },
    signedIn: true,
    prefs: { theme: 'dark', text: 100, motion: 'full', zone: 'Europe/Prague' },
    tasks: [
      {
        id: 1841, action: 'service.restart', title: 'Restart a service', args: { name: 'valkey' }, who: 'alice', how: 'console', started: now - 700,
        state: 'running', step: 3, steps: 4, words: 'installing 8.0.4 and restarting valkey', percent: 60, ended: null, status: null, note: null, changed: null,
      },
      {
        id: 1840, action: 'service.restart', title: 'Back up to nas', args: { name: 'nas' }, who: 'nightly schedule', how: 'schedule', started: now - 3600,
        state: 'running', step: 0, steps: 0, words: '', percent: null, ended: null, status: null, note: null, changed: null,
      },
      {
        id: 1839, action: 'service.restart', title: 'Get a certificate', args: { name: 'blog.example.org' }, who: 'certd', how: 'schedule', started: now - 7200,
        state: 'failed', step: 1, steps: 2, words: '', percent: null, ended: now - 7100, status: 1, note: "the name's IPv6 address points to another machine", changed: null,
      },
      {
        id: 1838, action: 'service.restart', title: 'Restart a service', args: { name: 'backup-sync' }, who: 'alice', how: 'console', started: now - 9000,
        state: 'done', step: 1, steps: 1, words: '', percent: 100, ended: now - 8990, status: 0, note: null, changed: true,
      },
    ],
    log: ['02:00:00 started', '02:00:31 192.168.1.20: no answer after 30 s'],
    actions: [
      { id: 'service.restart', title: 'Restart a service', area: 'services', changes: true, method: 'POST', path: '/api/1/services/{name}/restart', runs: 'as-administrator', long: false },
      { id: 'service.list', title: 'List the services', area: 'services', changes: false, method: 'GET', path: '/api/1/services', runs: 'as-the-person', long: false },
      { id: 'lab.fail', title: 'Fail on purpose', area: 'lab', changes: true, method: 'POST', path: '/api/1/lab/fail', runs: 'as-the-person', long: false },
    ],
    badge: null,
    offline: false,
    events: [],
  };
}

export function startServer({ port = 0, state = freshState() } = {}) {
  const streams = new Set();
  const send = (name, data) => {
    for (const response of streams) response.write(`event: ${name}\ndata: ${JSON.stringify(data)}\n\n`);
  };
  const json = (response, status, body, headers = {}) => {
    response.writeHead(status, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store', ...headers });
    response.end(JSON.stringify(body));
  };
  const error = (response, status, id, title, detail = '', extra = {}) => json(response, status, { error: { id, title, detail, ...extra } });

  const server = createServer(async (request, response) => {
    const url = new URL(request.url, 'http://localhost');
    const path = url.pathname;
    let body = '';
    for await (const chunk of request) body += chunk;
    const data = body ? JSON.parse(body) : {};

    if (path.startsWith('/api/')) {
      if (state.offline) {
        request.socket.destroy();
        return;
      }
      if (path === '/api/1/health') return json(response, 200, { ok: true });
      if (path === '/api/1/fingerprint') return json(response, 200, { host: state.session.host, fingerprint: 'AB:CD:EF:01:23:45' });
      if (path === '/api/1/sign-in') {
        state.signedIn = true;
        return json(response, 200, { ok: true });
      }
      if (!state.signedIn) return error(response, 401, 'ADM-107', 'You are signed out', 'The console has no session for you.');
      if (path === '/api/1/session') return json(response, 200, state.session);
      if (path === '/api/1/sign-out') {
        state.signedIn = false;
        return json(response, 200, { done: true });
      }
      if (path === '/api/1/events') {
        response.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-store' });
        response.write(': open\n\n');
        streams.add(response);
        request.on('close', () => streams.delete(response));
        return undefined;
      }
      if (path === '/api/1/account/preferences') {
        if (request.method === 'POST') {
          if (data.zone && !/^[A-Za-z_]+\/[A-Za-z_]+$|^UTC$/.test(data.zone)) {
            return error(response, 400, 'ADM-125', 'That time zone is not known', `${data.zone} is no time zone of this machine's database.`, { field: 'zone', fix: 'Pick one from the list.' });
          }
          Object.assign(state.prefs, data);
        }
        return json(response, 200, state.prefs);
      }
      if (path === '/api/1/unlock') {
        state.session.unlock_seconds = 900;
        send('unlock', { unlock_seconds: 900 });
        return json(response, 200, { unlock_seconds: 900 });
      }
      if (path === '/api/1/unlock/extend') {
        state.session.unlock_seconds = 900;
        return json(response, 200, { unlock_seconds: 900 });
      }
      if (path === '/api/1/lock') {
        state.session.unlock_seconds = null;
        // admind tells the stream of a lock as it does of an unlock.
        send('unlock', { unlock_seconds: null });
        return json(response, 200, { unlock_seconds: null });
      }
      if (path === '/api/1/actions') return json(response, 200, state.actions);
      if (path === '/api/1/show') {
        return json(response, 200, {
          command: `elevate service ${data.args?.name ?? 'nginx'} restart`,
          api: `curl -X POST https://duke-nas:9090/api/1/services/${data.args?.name ?? 'nginx'}/restart`,
          ansible: `- name: Restart ${data.args?.name ?? 'nginx'}\n  dukovo.admin.service:\n    name: ${data.args?.name ?? 'nginx'}\n    state: restarted`,
          files: ['/system/settings/spark/services/backup-sync.toml'],
        });
      }
      if (path === '/api/1/changes') return json(response, 200, { changes: [{ id: 7, at: 1, who: 'alice', how: 'console', files: [{ path: '/system/settings/spark/services/backup-sync.toml' }] }] });
      if (path === '/api/1/changes/7') {
        return json(response, 200, {
          id: 7,
          files: [{
            path: '/system/settings/spark/services/backup-sync.toml',
            diff: '--- backup-sync.toml (before)\n+++ backup-sync.toml (after)\n@@ -1,3 +1,3 @@\n [limits]\n-memory = "256M"\n+memory = "512M"\n',
          }],
        });
      }
      if (path === '/api/1/tasks') return json(response, 200, { tasks: state.tasks });
      const one = /^\/api\/1\/tasks\/(\d+)$/.exec(path);
      if (one) {
        const task = state.tasks.find((candidate) => candidate.id === Number(one[1]));
        return task ? json(response, 200, { ...task, log: state.log }) : error(response, 404, 'ADM-120', 'No such task');
      }
      const stop = /^\/api\/1\/tasks\/(\d+)\/stop$/.exec(path);
      if (stop) {
        const task = state.tasks.find((candidate) => candidate.id === Number(stop[1]));
        task.state = 'stopped';
        task.ended = Math.floor(Date.now() / 1000);
        return json(response, 200, { stopping: true, task });
      }
      const restart = /^\/api\/1\/services\/([^/]+)\/restart$/.exec(path);
      if (restart) {
        if (!state.session.unlock_seconds) return error(response, 403, 'ADM-106', 'The console is locked', 'Unlock it to make changes.');
        return json(response, 200, { done: true, changed: true, output: [] });
      }
      if (path === '/api/1/lab/fail') {
        return error(response, 500, 'ADM-123', 'The command did not work', 'The command ended with status 1.', {
          fix: 'Read the program\'s own words under the message.',
          command: { argv: ['lab', 'fail'], status: 1, stderr: 'lab: it failed on purpose\nSecond line' },
        });
      }
      if (path === '/api/1/terminals') return json(response, 200, { terminals: state.terminals ?? [] });
      if (path === '/api/1/account/second-step') return json(response, 200, { app: null, passkeys: [], needed: false });
      return error(response, 404, 'ADM-116', 'No such route');
    }

    // The help pages come from docs/errors, as the build copies them.
    if (path === '/help/errors/index.json') {
      // The build gives each page the README's area text up to its first colon.
      const areas = new Map([...readFileSync(join(ERRORS, 'README.md'), 'utf8').matchAll(/^\| `([A-Z]+)` \| ([^|:]*)/gm)].map((found) => [found[1], found[2].trim()]));
      const index = readdirSync(ERRORS).filter((name) => /^[A-Z]+-\d+\.md$/.test(name)).map((name) => {
        const id = name.replace('.md', '');
        const title = /^#\s+[A-Z]+-\d+:\s*(.*)$/m.exec(readFileSync(join(ERRORS, name), 'utf8'))?.[1] ?? id;
        return { id, title, area: areas.get(id.split('-')[0]) ?? id.split('-')[0] };
      });
      response.writeHead(200, { 'Content-Type': 'application/json' });
      response.end(JSON.stringify(index));
      return;
    }
    const help = /^\/help\/errors\/([A-Z]+-\d+)\.md$/.exec(path);
    if (help) {
      try {
        response.writeHead(200, { 'Content-Type': 'application/octet-stream' });
        response.end(readFileSync(join(ERRORS, `${help[1]}.md`)));
      } catch {
        response.writeHead(404);
        response.end();
      }
      return;
    }

    // The palette is another change's module; a page that looks at the
    // header answers for it here, so the find control is drawn.
    if (path === '/palette/palette.js' && state.palette !== false) {
      response.writeHead(200, { 'Content-Type': 'text/javascript' });
      response.end(request.method === 'HEAD' ? '' : 'export function togglePalette() { document.title = "palette"; }\n');
      return;
    }

    // A path with a dot is a file or nothing; any other path is the shell.
    const clean = normalize(path).replace(/^(\.\.[/\\])+/, '');
    const file = join(ADMIN, clean);
    if (path.includes('.') && !file.includes('/tests/')) {
      try {
        if (statSync(file).isFile()) {
          response.writeHead(200, { 'Content-Type': TYPES[extname(file)] ?? 'application/octet-stream', 'Cache-Control': 'no-cache' });
          if (request.method === 'HEAD') response.end();
          else response.end(readFileSync(file));
          return;
        }
      } catch {
        // Falls through to the 404.
      }
      response.writeHead(404);
      response.end();
      return;
    }
    response.writeHead(200, { 'Content-Type': 'text/html', 'Cache-Control': 'no-cache' });
    response.end(readFileSync(join(ADMIN, 'index.html')));
  });
  return new Promise((resolve) => {
    server.listen(port, '127.0.0.1', () => resolve({
      server, state, send, port: server.address().port, close: () => { for (const stream of streams) stream.end(); server.close(); },
    }));
  });
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const { port } = await startServer({ port: Number(process.argv[2] ?? 8099) });
  console.log(`The console's pages are at http://127.0.0.1:${port}/`);
}
