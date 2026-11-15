// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A stand-in for admind's terminals, for the tests of the Terminal page and
// for looking at it in a browser: the routes of design 13.6 and the socket
// of 13.7 on `node:http`'s upgrade, with the frames of RFC 6455 written
// here (no package). Panes hold a ring of output with offsets, a replay and
// `since` work as admind's do, and a pane's "shell" echoes what is typed
// and runs `echo`, `exit` and `big N` (N lines of output), which is all a
// test of the page needs. Never installed: install_admin leaves tests/ out.
//
//   const fake = await startFake({ root });     root: a folder to serve (the console's)
//   fake.addTerminal({ name: 'logs', panes: 2 })
//   fake.write(paneId, 'text')                  output, as the shell would write it
//   fake.drop(1006)                             closes every socket, as a network fault does
//   await fake.close()

import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { extname, join, normalize } from 'node:path';

import { decodeClient, encodeServer } from './term-server-wire.mjs';

const GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';
const SUBPROTOCOL = 'dukovo-terminal-1';
const CHUNK = 16 * 1024;
const RING = 64 * 1024;
const SOCKETS_MOST = 4;
const TYPES = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml', '.json': 'application/json' };

// A frame to send: servers never mask.
function frame(opcode, payload = Buffer.alloc(0)) {
  const length = payload.length;
  const head = length < 126 ? Buffer.from([0x80 | opcode, length])
    : length < 65536 ? Buffer.from([0x80 | opcode, 126, length >> 8, length & 255])
      : Buffer.concat([Buffer.from([0x80 | opcode, 127, 0, 0, 0, 0]), Buffer.from([length >>> 24, (length >> 16) & 255, (length >> 8) & 255, length & 255])]);
  return Buffer.concat([head, payload]);
}

// One socket: reads masked frames, joins fragments, and hands messages on.
class Peer {
  buffer = Buffer.alloc(0);

  parts = [];

  first = 0;

  open = true;

  constructor(socket, server) {
    this.socket = socket;
    this.server = server;
    this.panes = new Set();
    socket.on('data', (chunk) => this.read(chunk));
    socket.on('close', () => this.gone());
    socket.on('error', () => socket.destroy());
  }

  send(opcode, payload) {
    if (this.open) this.socket.write(frame(opcode, payload));
  }

  text(string) {
    this.send(1, Buffer.from(string));
  }

  binary(bytes) {
    this.send(2, Buffer.from(bytes));
  }

  close(code = 1000) {
    if (!this.open) return;
    this.send(8, Buffer.from([code >> 8, code & 255]));
    this.open = false;
    this.socket.end();
  }

  gone() {
    this.open = false;
    this.server.peers.delete(this);
    for (const pane of this.panes) this.server.state.panes.get(pane)?.attached.delete(this);
  }

  read(chunk) {
    this.buffer = Buffer.concat([this.buffer, chunk]);
    for (;;) {
      const buffer = this.buffer;
      if (buffer.length < 2) return;
      const masked = (buffer[1] & 0x80) !== 0;
      let length = buffer[1] & 0x7f;
      let at = 2;
      if (length === 126) {
        if (buffer.length < 4) return;
        length = buffer.readUInt16BE(2);
        at = 4;
      } else if (length === 127) {
        if (buffer.length < 10) return;
        length = Number(buffer.readBigUInt64BE(2));
        at = 10;
      }
      if (!masked) return this.close(1002);
      if (buffer.length < at + 4 + length) return;
      const mask = buffer.subarray(at, at + 4);
      const payload = Buffer.from(buffer.subarray(at + 4, at + 4 + length));
      for (let i = 0; i < payload.length; i++) payload[i] ^= mask[i % 4];
      this.buffer = buffer.subarray(at + 4 + length);
      this.frame(buffer[0] & 0x0f, (buffer[0] & 0x80) !== 0, payload);
    }
  }

  frame(opcode, fin, payload) {
    if (opcode === 8) return this.close(1000);
    if (opcode === 9) return this.send(10, payload);
    if (opcode === 10) return undefined;
    if (opcode !== 0) this.first = opcode;
    this.parts.push(payload);
    if (!fin) return undefined;
    const whole = Buffer.concat(this.parts);
    this.parts = [];
    return this.server.message(this, this.first === 1 ? whole.toString('utf8') : new Uint8Array(whole));
  }
}

/** Starts the fake on a free port: `{ port, origin, token, state, peers, log, ... }`. */
export async function startFake({ root = null, token = 'csrf-token-0123456789abcdef', session = {} } = {}) {
  const state = { terminals: new Map(), panes: new Map(), next: 1 };
  const peers = new Set();
  const fake = {
    state, peers, token, log: [], acks: [], answers: [], refusals: [], sessionEnded: false, shell: true, session, upgrades: [],
  };

  const pane = (id) => state.panes.get(id);

  // Output to a pane: kept in the ring, and sent to each socket that shows it.
  function write(id, data) {
    const p = pane(id);
    if (!p) return;
    const bytes = typeof data === 'string' ? Buffer.from(data) : Buffer.from(data);
    for (let at = 0; at < bytes.length; at += CHUNK) {
      const piece = bytes.subarray(at, at + CHUNK);
      p.ring.push({ offset: p.offset, bytes: piece });
      p.offset += piece.length;
      for (const peer of p.attached) peer.binary(encodeServer({ type: 'output', pane: id, offset: p.offset - piece.length, bytes: new Uint8Array(piece) }));
    }
    while (p.ring.length > 1 && p.offset - p.ring[0].offset > RING) p.ring.shift();
  }

  function addPane(terminal, { admin = false, cols = 80, rows = 24 } = {}) {
    const id = state.next++;
    state.panes.set(id, {
      id, terminal: terminal.id, admin, cols, rows, ring: [], offset: 0, modes: '', ended: null, attached: new Set(), line: '', started: Date.now() / 1000,
    });
    terminal.panes.push(id);
    if (fake.shell) write(id, '$ ');
    return id;
  }

  function addTerminal({ name = 'alice@duke-nas', panes = 1, admin = false, layout = 'one', sizes = [] } = {}) {
    const terminal = {
      id: state.next++, name, made: Math.floor(Date.now() / 1000), used: Math.floor(Date.now() / 1000), from: '127.0.0.1',
      keep_days: 7, layout, sizes, panes: [],
    };
    state.terminals.set(terminal.id, terminal);
    for (let i = 0; i < panes; i++) addPane(terminal, { admin: admin && i === 0 });
    return terminal;
  }

  const json = (terminal) => ({
    id: terminal.id, name: terminal.name, made: terminal.made, used: terminal.used, from: terminal.from, keep_days: terminal.keep_days,
    layout: terminal.layout, sizes: terminal.sizes,
    panes: terminal.panes.map(pane).filter((p) => p && !p.ended).map((p) => ({
      id: p.id, admin: p.admin, cols: p.cols, rows: p.rows, busy: p.busy ?? false, started: p.started, attached: p.attached.size,
    })),
  });

  function endPane(id, why = 'ended', status) {
    const p = pane(id);
    if (!p || p.ended) return;
    p.ended = why;
    for (const peer of p.attached) peer.text(encodeServer({ type: 'ended', pane: id, why, ...(status === undefined ? {} : { status }) }));
    p.attached.clear();
    const terminal = state.terminals.get(p.terminal);
    if (terminal && terminal.panes.every((one) => pane(one)?.ended)) state.terminals.delete(terminal.id);
    changed(terminal?.id);
  }

  function changed(id) {
    for (const peer of peers) peer.text(encodeServer({ type: 'changed', terminal: id }));
  }

  const failure = (peer, id, number, title, detail) => peer.text(encodeServer({ type: 'error', error: { id: `ADM-${number}`, title, detail }, pane: id }));

  function attach(peer, { pane: id, cols, rows, since }) {
    const p = pane(id);
    if (!p || p.ended) return failure(peer, id, 131, 'There is no such terminal', 'No terminal has that pane.');
    const first = p.ring[0]?.offset ?? p.offset;
    const replay = since === undefined || since < first || since > p.offset;
    const from = replay ? first : since;
    p.attached.add(peer);
    peer.panes.add(id);
    p.cols = cols;
    p.rows = rows;
    const terminal = state.terminals.get(p.terminal);
    peer.text(encodeServer({
      type: 'attached', pane: id, terminal: p.terminal, name: terminal.name, admin: p.admin, cols, rows, offset: from, live: p.offset, replay, ...(replay ? { modes: p.modes } : {}),
    }));
    for (const part of p.ring) {
      if (part.offset + part.bytes.length <= from) continue;
      const skip = Math.max(0, from - part.offset);
      peer.binary(encodeServer({ type: 'output', pane: id, offset: part.offset + skip, bytes: new Uint8Array(part.bytes.subarray(skip)) }));
    }
  }

  // The "shell": echoes, and runs a line at Enter.
  function type(id, bytes) {
    const p = pane(id);
    if (!p || p.ended || !fake.shell) return;
    for (const byte of bytes) {
      if (byte === 13) {
        const line = p.line.trim();
        p.line = '';
        write(id, '\r\n');
        if (line.startsWith('echo ')) write(id, `${line.slice(5)}\r\n`);
        else if (line === 'exit') return endPane(id, 'exited', 0);
        else if (line.startsWith('big ')) for (let i = 1; i <= Number(line.slice(4)); i++) write(id, `line ${i}\r\n`);
        else if (line !== '') write(id, `fake: ${line}: not found\r\n`);
        write(id, '$ ');
      } else if (byte === 127) {
        if (p.line !== '') {
          p.line = p.line.slice(0, -1);
          write(id, '\b \b');
        }
      } else if (byte >= 32 || byte === 9) {
        p.line += String.fromCharCode(byte);
        write(id, Uint8Array.of(byte));
      }
    }
  }

  fake.message = (peer, data) => {
    const read = decodeClient(data);
    if (!read.ok) return peer.close(1008);
    const { message } = read;
    fake.log.push(message);
    if (message.type === 'attach') attach(peer, message);
    else if (message.type === 'detach') {
      pane(message.pane)?.attached.delete(peer);
      peer.panes.delete(message.pane);
    } else if (message.type === 'resize') {
      const p = pane(message.pane);
      if (p?.attached.has(peer)) Object.assign(p, { cols: message.cols, rows: message.rows });
      else failure(peer, message.pane, 131, 'There is no such terminal', 'This socket does not show that pane.');
    } else if (message.type === 'ack') fake.acks.push({ pane: message.pane, offset: message.offset });
    else if (message.type === 'input') type(message.pane, message.bytes);
    else if (message.type === 'answer') fake.answers.push({ pane: message.pane, text: message.text });
    return undefined;
  };

  // The routes of design 13.6.
  async function route(request, response, url, body) {
    const send = (status, answer) => {
      response.writeHead(status, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
      response.end(JSON.stringify(answer));
    };
    const error = (status, number, title, detail) => send(status, { error: { id: `ADM-${number}`, title, detail } });
    const path = url.pathname;
    if (fake.sessionEnded) return error(401, 107, 'The session has ended', 'Sign in again.');
    if (request.method === 'GET' && path === '/api/1/session') {
      return send(200, {
        name: 'alice', full_name: 'Alice', admin: false, host: 'duke-nas', csrf: token, unlock_seconds: null, needs_step: false, kinds: ['password'], terminal: true, ...session,
      });
    }
    if (request.method === 'GET' && path === '/api/1/terminals') {
      return send(200, { terminals: [...state.terminals.values()].map(json), most: 32, sockets: { open: peers.size, most: SOCKETS_MOST } });
    }
    if (request.method === 'POST' && request.headers['x-dukovo-csrf'] !== token) return error(403, 109, 'That did not come from the console', 'The token is missing.');
    const data = body ? JSON.parse(body) : {};
    if (request.method === 'POST' && path === '/api/1/terminals') {
      if (state.terminals.size >= 32) return error(409, 130, 'Too many terminals', 'You have 32 terminals. End one on the Sessions page.');
      if (data.admin && !fake.unlocked) return error(403, 106, 'The console is locked', 'Unlock it for an administrator\'s terminal.');
      const terminal = addTerminal({ name: data.name ?? (data.admin ? 'root@duke-nas' : 'alice@duke-nas'), admin: data.admin });
      pane(terminal.panes[0]).cols = data.cols;
      pane(terminal.panes[0]).rows = data.rows;
      changed(terminal.id);
      return send(200, { terminal: json(terminal) });
    }
    const found = /^\/api\/1\/terminals\/(\d+)\/(panes|layout|rename|end)$/.exec(path);
    if (request.method === 'POST' && found) {
      const terminal = state.terminals.get(Number(found[1]));
      if (!terminal) return error(404, 131, 'There is no such terminal', 'It has ended, or it is not yours.');
      if (found[2] === 'panes') {
        if (terminal.panes.length >= 4) return error(409, 130, 'Too many terminals', 'A terminal has 4 panes at most.');
        const id = addPane(terminal, { admin: data.admin, cols: data.cols, rows: data.rows });
        pane(id).cols = data.cols;
        pane(id).rows = data.rows;
        terminal.layout = data.layout ?? (terminal.layout === 'one' ? 'columns' : terminal.layout);
      } else if (found[2] === 'layout') Object.assign(terminal, { layout: data.layout, sizes: data.sizes ?? [] });
      else if (found[2] === 'rename') terminal.name = data.name;
      else {
        for (const id of [...terminal.panes]) endPane(id, 'ended');
        return send(200, { done: true });
      }
      changed(terminal.id);
      return send(200, { terminal: json(terminal) });
    }
    const ending = /^\/api\/1\/terminals\/panes\/(\d+)\/end$/.exec(path);
    if (request.method === 'POST' && ending) {
      if (!pane(Number(ending[1]))) return error(404, 131, 'There is no such terminal', 'It has ended, or it is not yours.');
      endPane(Number(ending[1]), 'ended');
      return send(200, { done: true });
    }
    return fake.other ? fake.other(request, response, url, body) : error(404, 116, 'Nothing is at that address', path);
  }

  const server = createServer(async (request, response) => {
    const url = new URL(request.url, 'http://x');
    let body = '';
    for await (const chunk of request) body += chunk;
    if (url.pathname.startsWith('/api/')) return route(request, response, url, body);
    if (!root) {
      response.writeHead(404).end();
      return undefined;
    }
    const clean = normalize(url.pathname).replace(/^(\.\.[/\\])+/, '');
    const file = await readFile(join(root, clean)).then((content) => ({ content, type: TYPES[extname(clean)] }), () => null);
    if (file?.type) response.writeHead(200, { 'Content-Type': file.type, 'Cache-Control': 'no-cache' }).end(file.content);
    else response.writeHead(200, { 'Content-Type': 'text/html' }).end(await readFile(join(root, 'index.html')));
    return undefined;
  });

  server.on('upgrade', (request, socket) => {
    const refuse = (status, id) => {
      const body = JSON.stringify({ error: { id, title: 'Refused', detail: 'The socket was refused.' } });
      socket.end(`HTTP/1.1 ${status} Refused\r\nContent-Type: application/json\r\nContent-Length: ${Buffer.byteLength(body)}\r\nConnection: close\r\n\r\n${body}`);
    };
    const offered = String(request.headers['sec-websocket-protocol'] ?? '').split(',').map((one) => one.trim());
    fake.upgrades.push({ origin: request.headers.origin, protocols: offered, site: request.headers['sec-fetch-site'] });
    if (fake.refusals.length > 0) return refuse(fake.refusals.shift(), 'ADM-130');
    if (request.url !== '/api/1/terminal' || !offered.includes(SUBPROTOCOL) || !offered.includes(`csrf.${token}`)) return refuse(403, 'ADM-109');
    // A page of another origin is refused; a client that sends no Origin (Node's own) is let in.
    if (request.headers.origin !== undefined && request.headers.origin !== `http://${request.headers.host}`) return refuse(403, 'ADM-109');
    if (peers.size >= SOCKETS_MOST) return refuse(429, 'ADM-130');
    const accept = createHash('sha1').update(`${request.headers['sec-websocket-key']}${GUID}`).digest('base64');
    socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\nSec-WebSocket-Protocol: ${SUBPROTOCOL}\r\n\r\n`);
    const peer = new Peer(socket, fake);
    peers.add(peer);
    return undefined;
  });

  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address();
  Object.assign(fake, {
    port,
    origin: `http://127.0.0.1:${port}`,
    addTerminal,
    addPane,
    write,
    endPane,
    changed,
    pane,
    json,
    /** Sends `text` frames and closes every socket with `code`, as a fault in the network does. */
    drop(code = 1006) {
      for (const peer of peers) {
        if (code === 1006) peer.socket.destroy();
        else peer.close(code);
      }
    },
    /** Sends a ping to every socket. */
    ping() {
      for (const peer of peers) peer.send(9, Buffer.from('p'));
    },
    close: () => new Promise((resolve) => {
      for (const peer of peers) peer.socket.destroy();
      server.close(resolve);
      server.closeAllConnections?.();
    }),
  });
  return fake;
}
