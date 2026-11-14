// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The fake API of devserver.mjs with what the palette reads and runs added:
// the actions admind lists (from its registry, with their schemas), the
// services Spark runs, the certificates certd keeps, an explanation, the
// command that `POST /api/1/show` gives for each action, and the changes,
// which need the console to be unlocked and refuse a name that admind's
// check refuses. For looking at the palette in a browser; never installed.
//
//   node userspace/admin/tests/palette-server.mjs [port]

import { startServer } from './devserver.mjs';
import { listedActions } from './registry.mjs';

const SERVICES = [
  { name: 'nginx', description: 'Web server', state: 'running', enabled: true },
  { name: 'diskhealth', description: 'Watches the disks', state: 'running', enabled: true },
  { name: 'backup-sync', description: 'Copies to the NAS', state: 'failed', enabled: true },
  { name: 'valkey', description: 'Key-value store', state: 'stopped', enabled: false },
  { name: 'admind', description: 'The console', state: 'running', enabled: true },
  { name: 'authd', description: 'Signs people in', state: 'running', enabled: true },
];

const CERTIFICATES = [
  { name: 'this-machine', kind: 'machine', days_left: 59, used_by: ['admind'], trusted: true },
  { name: 'photos.example.org', kind: 'acme', days_left: 9, used_by: ['nginx'], trusted: true },
];

const COMMANDS = {
  'service.list': 'service list --json',
  'service.explain': (args) => `service ${args.name} explain --json`,
  'certificate.list': 'cert list --json',
  'certificate.show': (args) => `cert show ${args.name} --json`,
};

/** The command of an action with arguments, as `admin` shows it: the verb after the id's dot, for the services' changes. */
function commandOf(id, args) {
  const known = COMMANDS[id];
  if (typeof known === 'string') return known;
  if (known) return known(args);
  const [area, verb] = id.split('.');
  if (area === 'service') return `elevate service ${args.name} ${verb}`;
  if (area === 'certificate') return `elevate cert ${verb} ${args.name}`;
  return `admin ${id}${Object.keys(args).length > 0 ? ` ${JSON.stringify(args)}` : ''}`;
}

/**
 * Starts the server of devserver.mjs with the palette's routes in front of
 * it. `state.actions` is the registry's list; `state.services` and
 * `state.certificates` are what the two lists answer.
 */
export async function startPaletteServer({ port = 0 } = {}) {
  const served = await startServer({ port });
  const { server, state } = served;
  state.actions = listedActions();
  state.services = SERVICES.map((one) => ({ ...one }));
  state.certificates = CERTIFICATES.map((one) => ({ ...one }));
  state.palette = false;
  state.shown = [];
  const next = server.listeners('request')[0];
  server.removeAllListeners('request');

  const json = (response, status, body) => {
    response.writeHead(status, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
    response.end(JSON.stringify(body));
  };
  const fail = (response, status, id, title, detail, extra = {}) => json(response, status, { error: { id, title, detail, ...extra } });
  const body = async (request) => {
    let text = '';
    for await (const chunk of request) text += chunk;
    return text ? JSON.parse(text) : {};
  };

  server.on('request', async (request, response) => {
    const url = new URL(request.url, 'http://localhost');
    const path = url.pathname;
    if (!path.startsWith('/api/1/') || !state.signedIn || state.offline) return next(request, response);

    if (path === '/api/1/services' && request.method === 'GET') return json(response, 200, { show_version: 1, services: state.services });
    if (path === '/api/1/certificates' && request.method === 'GET') return json(response, 200, state.certificates);
    const explain = /^\/api\/1\/services\/([^/]+)\/explain$/.exec(path);
    if (explain) {
      const service = state.services.find((one) => one.name === decodeURIComponent(explain[1]));
      if (!service) return fail(response, 404, 'ADM-123', 'The command did not work', 'No service has that name.');
      // `service NAME explain` ends with 3 for a service that does not run well, which admind answers as a command that did not work.
      if (service.state !== 'running') return fail(response, 422, 'ADM-123', 'The command did not work', 'service ended with status 3');
      // The answer of `service NAME explain --json`: the sentence, the services followed, the root of the trouble and what to do.
      return json(response, 200, { sentence: `${service.name} is running as process 57, ready since 10:42:03.`, chain: [service.name], root: service.name, fix: null });
    }
    const shown = /^\/api\/1\/certificates\/([^/]+)$/.exec(path);
    if (shown && request.method === 'GET') {
      const certificate = state.certificates.find((one) => one.name === decodeURIComponent(shown[1]));
      return certificate ? json(response, 200, certificate) : fail(response, 404, 'ADM-123', 'The command did not work', 'No certificate has that name.');
    }
    if (path === '/api/1/show' && request.method === 'POST') {
      const data = await body(request);
      state.shown.push(data);
      const args = data.args ?? {};
      const command = commandOf(data.action, args);
      return json(response, 200, {
        command,
        api: `curl -X POST https://duke-nas:9090/api/1/${data.action.replace('.', '/')}`,
        ansible: `- name: ${data.action}\n  dukovo.admin.action:\n    id: ${data.action}`,
        files: [],
      });
    }
    const change = /^\/api\/1\/(services|certificates)\/([^/]+)\/(start|stop|restart|enable|disable|renew)$/.exec(path);
    if (change && request.method === 'POST') {
      const name = decodeURIComponent(change[2]);
      if (!state.session.unlock_seconds) return fail(response, 403, 'ADM-106', 'The console is locked', 'Unlock it to make changes.');
      if (!/^[A-Za-z0-9_][A-Za-z0-9_.-]*$/.test(name) || name.includes('..')) {
        return fail(response, 400, 'ADM-125', 'That name cannot be used', `${name} is not a name this action takes.`, { field: 'name', fix: 'Use letters, digits, hyphens, underscores and dots.' });
      }
      if (['admind', 'authd'].includes(name) && change[3] !== 'start' && change[3] !== 'enable') {
        return fail(response, 400, 'ADM-125', 'The console runs on this service', `The console runs on ${name}: stop or restart it at a terminal.`, { field: 'name' });
      }
      state.changed = [...(state.changed ?? []), `${change[1]}/${name}/${change[3]}`];
      return json(response, 200, { done: true, changed: true, output: [] });
    }
    const read = /^\/api\/1\/(console\/about|checks\/last|metrics\/now)$/.exec(path);
    if (read) return json(response, 200, { release: '2026.11', reach: 'private' });
    return next(request, response);
  });
  return served;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const { port } = await startPaletteServer({ port: Number(process.argv[2] ?? 8098) });
  console.log(`The console, with the palette's routes, is at http://127.0.0.1:${port}/`);
}
