// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The dev server of devserver.mjs with the answers the AI clients page
// reads and the changes it makes, for looking at the page in a browser
// (design 17: each board checked in dark and light, at 1280 and 390 wide).
// Never installed. It starts with the clients, the request and the history
// of clients-fixtures.mjs and keeps them as the page changes them: a client
// that is added is listed, with a token when it connects over HTTPS; a
// request that is approved or refused, or whose client is removed, leaves
// the list and is in the history with what became of it; and each tells
// the page's stream how many wait, as admind does. The fixtures' "now" is
// `NOW` of clients-fixtures.mjs: a browser that looks at the page sets its
// clock there.
//
//   node userspace/admin/tests/clients-server.mjs [port]
//
// `state.clients` holds it all, for a test to change before a reload, and
// `ask(started, asking)` files a request, as a client's call of the tool
// ask does.

import { randomBytes } from 'node:crypto';

import {
  about, areas, AT, every, history, machine, requests,
} from './clients-fixtures.mjs';
import { freshState, startServer } from './devserver.mjs';

/** The actions the page runs, as `GET /api/1/actions` lists them. */
const ACTIONS = [
  { id: 'console.clients', title: 'List every account\'s AI clients', area: 'console', changes: false, method: 'GET', path: '/api/1/console/clients', runs: 'as-administrator', long: false },
  { id: 'console.client-remove', title: 'Remove an AI client of an account', area: 'console', changes: true, method: 'POST', path: '/api/1/console/clients/{owner}/{name}/remove', runs: 'as-administrator', long: false },
];

/** What `POST /api/1/show` answers for the page's own actions; the dev server answers the rest. */
function shown(action, args) {
  const commands = {
    'console.clients': 'elevate admin mcp list --all --json',
    'console.client-remove': `elevate admin mcp remove ${args?.name} --of ${args?.owner}`,
  };
  const command = commands[action];
  if (command === undefined) return null;
  return {
    command, api: `curl https://duke-nas:9090/api/1/console/clients`, ansible: `- name: ${action}\n  ansible.builtin.command: ${command.replace('elevate ', '')}\n  become: true`, files: [],
  };
}

/** The state of the fake API: devserver's, signed in as the fixtures' administrator, with what the page reads. */
export function clientsState() {
  const state = freshState();
  Object.assign(state.session, { name: 'chase', full_name: 'Chase' });
  state.actions.push(...ACTIONS.map((action) => ({ ...action })));
  state.clients = {
    areas: areas().areas,
    all: every().clients,
    waiting: requests().requests,
    history: history().entries,
    // The tokens given, each with whose client it lets in: `{owner, name}`.
    tokens: new Map(),
    calls: [],
  };
  return state;
}

/** Tells the page's stream how many requests wait. */
const tell = (started) => started.send('clients', { waiting: started.state.clients.waiting.length });

/**
 * Files a request for the client `asking.client` of `asking.owner`, as its
 * call of the tool ask does, and gives its number. `asking` holds the rest
 * of a request too: `action`, `title`, `area`, `args`, `reason`, `command`
 * and `files`.
 */
export function ask(started, asking) {
  const data = started.state.clients;
  const numbers = [...data.waiting, ...data.history].map((one) => one.request ?? 0);
  const request = {
    request: Math.max(0, ...numbers) + 1, files: [], ...asking, asked: AT, lapses: AT + 3600,
  };
  data.waiting.push(request);
  data.history.unshift({
    at: AT, owner: request.owner, client: request.client, kind: 'ask', request: request.request, action: request.action, title: request.title, state: 'waiting',
  });
  tell(started);
  return request.request;
}

/** Takes the requests that `gone` picks off the list, as `state` with `more`, and tells the stream. */
function settle(started, gone, state, more = {}) {
  const data = started.state.clients;
  for (const request of data.waiting.filter(gone)) {
    const entry = data.history.find((one) => one.kind === 'ask' && one.request === request.request);
    if (entry) Object.assign(entry, { state, ...more });
  }
  data.waiting = data.waiting.filter((request) => !gone(request));
  tell(started);
}

/** Starts the server; gives what `startServer` gives. */
export async function startClients({ port = 0, state = clientsState() } = {}) {
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

  /** Removes the client `name` of `owner`, with what it had waiting; gives whether there was one. */
  function remove(owner, name) {
    const data = state.clients;
    const of = (one) => one.owner === owner && (one.name ?? one.client) === name;
    if (!data.all.some(of)) return false;
    data.all = data.all.filter((one) => !of(one));
    for (const [token, client] of data.tokens) if (of(client)) data.tokens.delete(token);
    settle(started, of, 'lapsed');
    return true;
  }

  /** Answers the routes of the AI clients page, and gives true when it did. */
  async function mine(request, response) {
    const { pathname } = new URL(request.url, 'http://localhost');
    const ours = ['/api/1/clients/', '/api/1/account/clients', '/api/1/console/clients'].some((start) => pathname.startsWith(start))
      || ['/api/1/check', '/api/1/show', '/api/1/console/about', '/api/1/certificates/this-machine'].includes(pathname);
    if (!ours || !state.signedIn || state.offline) return false;
    const data = state.clients;
    const get = request.method === 'GET';
    let body = '';
    for await (const chunk of request) body += chunk;
    const args = body ? JSON.parse(body) : {};
    const owner = state.session.name;

    if (pathname === '/api/1/show') {
      const answer = shown(args.action, args.args);
      if (answer) return json(response, 200, answer);
      // The body is read by now, so the dev server cannot be asked: this stands in for its answer.
      return json(response, 200, {
        command: `elevate service ${args.args?.name ?? 'nginx'} ${String(args.action).split('.').pop()}`, api: '', ansible: '', files: [],
      });
    }
    if (pathname === '/api/1/check') return json(response, 200, { changed: true, now: null });
    if (get && pathname === '/api/1/console/about') return json(response, 200, about());
    if (get && pathname === '/api/1/certificates/this-machine') return json(response, 200, machine());
    if (get && pathname === '/api/1/account/clients') return json(response, 200, { clients: data.all.filter((one) => one.owner === owner) });

    // What only an administrator is answered.
    if (!state.session.admin) return error(response, 403, 'ADM-122', 'Only administrators can do that', `${owner} is not an administrator, and AI clients are an administrator's to let in.`);
    if (get && pathname === '/api/1/clients/areas') return json(response, 200, { areas: data.areas });
    if (get && pathname === '/api/1/clients/requests') return json(response, 200, { requests: data.waiting });
    if (get && pathname === '/api/1/clients/history') return json(response, 200, { entries: data.history });
    if (get && pathname === '/api/1/console/clients') return state.session.unlock_seconds ? json(response, 200, { clients: data.all }) : locked(response);
    if (get) return false;

    data.calls.push({ path: pathname, args });
    const answered = /^\/api\/1\/clients\/requests\/(\d+)\/(approve|refuse)$/.exec(pathname);
    if (answered) {
      const [, id, verb] = answered;
      if (verb === 'approve' && !state.session.unlock_seconds) return locked(response);
      const one = (waiting) => waiting.request === Number(id);
      if (!data.waiting.some(one)) return error(response, 404, 'ADM-145', 'No such request', `request ${id} does not wait`);
      settle(started, one, verb === 'approve' ? 'done' : 'refused', verb === 'approve' ? { by: owner, ended: AT } : { by: owner });
      return json(response, 200, verb === 'approve' ? { done: true, changed: true, output: [] } : { done: true });
    }
    if (pathname === '/api/1/account/clients/add') {
      if (!state.session.unlock_seconds && !args.proof) return locked(response);
      if (data.all.some((one) => one.owner === owner && one.name === args.name)) {
        return error(response, 400, 'ADM-125', 'That value is not allowed', `${owner} already has an AI client named ${args.name}`);
      }
      data.all.push({
        owner, name: args.name, way: args.way, scopes: args.scopes, until: AT + args.days * 86_400, made: AT, used: 0, from: '', ended: false,
      });
      const token = args.way === 'https' ? `dkc_${owner}_${randomBytes(16).toString('hex')}` : null;
      if (token) data.tokens.set(token, { owner, name: args.name });
      return json(response, 200, { token });
    }
    if (pathname === '/api/1/account/clients/remove') {
      return remove(owner, args.name) ? json(response, 200, { done: true }) : error(response, 422, 'ADM-123', 'The command did not work', `${owner} has no AI client named ${args.name}`);
    }
    const of = /^\/api\/1\/console\/clients\/([^/]+)\/([^/]+)\/remove$/.exec(pathname);
    if (of) {
      if (!state.session.unlock_seconds) return locked(response);
      const [whose, name] = [decodeURIComponent(of[1]), decodeURIComponent(of[2])];
      return remove(whose, name)
        ? json(response, 200, { done: true, changed: true, output: [] })
        : error(response, 422, 'ADM-123', 'The command did not work', `${whose} has no AI client named ${name}`);
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
  const { port } = await startClients({ port: Number(process.argv[2] ?? 8099) });
  console.log(`The console's pages are at http://127.0.0.1:${port}/`);
}
