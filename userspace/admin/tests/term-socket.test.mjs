// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The page's socket client against a fake admind (term-fake-server.mjs): the
// handshake's subprotocols, attach and replay, `since` after a fault, typing,
// the acknowledgements, and the tries after one fails.

import assert from 'node:assert/strict';
import test from 'node:test';

import { openSocket } from '../pages/terminal/socket.js';
import { encodeServer } from './term-server-wire.mjs';
import { startFake } from './term-fake-server.mjs';

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(condition, what = 'the condition', ms = 3000) {
  const end = Date.now() + ms;
  while (!condition()) {
    if (Date.now() > end) assert.fail(`timed out waiting for ${what}`);
    await wait(5);
  }
}
const text = (chunks) => new TextDecoder().decode(Buffer.concat(chunks.map((chunk) => Buffer.from(chunk))));

// A fake, a client of it, and what the client was told.
async function setup(options = {}) {
  const fake = await startFake();
  const told = { statuses: [], changed: [], problems: [] };
  const client = openSocket({
    location: { protocol: 'http:', host: `127.0.0.1:${fake.port}` },
    token: fake.token,
    onStatus: (status) => told.statuses.push(status),
    onChanged: (id) => told.changed.push(id),
    onProblem: (words) => told.problems.push(words),
    delay: () => 10,
    ...options,
  });
  const done = async () => {
    client.stop();
    await fake.close();
  };
  return { fake, client, told, done };
}

// A pane that records what the client is told of it.
function watch(client, pane, { since = null, cols = 80, rows = 24 } = {}) {
  const seen = { attached: [], output: [], ended: [], detached: [], errors: [] };
  client.attach(pane, { cols, rows, since }, {
    attached: (message) => seen.attached.push(message),
    output: (bytes, offset) => seen.output.push({ bytes: bytes.slice(), offset }),
    ended: (message) => seen.ended.push(message),
    detached: (message) => seen.detached.push(message),
    error: (error) => seen.errors.push(error),
  });
  return seen;
}
const outputText = (seen) => text(seen.output.map((one) => one.bytes));

test('the socket offers the console\'s protocol and the session\'s token, and says when it is open', async () => {
  const { fake, told, done } = await setup();
  await until(() => told.statuses.some((status) => status.state === 'open'), 'open');
  assert.deepEqual(told.statuses.map((status) => status.state), ['connecting', 'open']);
  assert.deepEqual(fake.upgrades[0].protocols, ['dukovo-terminal-1', `csrf.${fake.token}`]);
  await done();
});

test('attaching a pane replays it, and what it writes after comes in order', async () => {
  const { fake, client, done } = await setup();
  const { panes: [pane] } = fake.addTerminal({ panes: 1 });
  fake.write(pane, 'hello\r\n');
  const seen = watch(client, pane);
  await until(() => seen.attached.length === 1 && outputText(seen) === '$ hello\r\n', 'the replay');
  assert.deepEqual(seen.attached[0], {
    type: 'attached', pane, terminal: seen.attached[0].terminal, name: 'alice@duke-nas', admin: false, cols: 80, rows: 24, offset: 0, live: 9, replay: true, modes: '',
  });
  fake.write(pane, 'more');
  await until(() => outputText(seen).endsWith('more'), 'more');
  assert.deepEqual(seen.output.map((one) => one.offset), [0, 2, 9], 'the ring is replayed as it was written');
  assert.equal(client.shown(pane), true);
  await done();
});

test('a replay brings the modes the page puts its screen in first', async () => {
  const { fake, client, done } = await setup();
  const { panes: [pane] } = fake.addTerminal({ panes: 1 });
  fake.pane(pane).modes = '\u001b[?1049h\u001b[?25l';
  const seen = watch(client, pane);
  await until(() => seen.attached.length === 1);
  assert.equal(seen.attached[0].modes, '\u001b[?1049h\u001b[?25l');
  await done();
});

test('what is typed reaches the pane and its echo comes back', async () => {
  const { fake, client, done } = await setup();
  const { panes: [pane] } = fake.addTerminal({ panes: 1 });
  const seen = watch(client, pane);
  await until(() => client.shown(pane));
  assert.equal(client.input(pane, new Uint8Array(0)), true, 'nothing to send is sent as nothing');
  assert.equal(client.input(7.5, Uint8Array.of(97)), false, 'a pane that is not a number');
  assert.equal(client.input(pane, new TextEncoder().encode('echo hi\r')), true);
  await until(() => outputText(seen).includes('hi\r\n$ '), 'the answer');
  assert.equal(outputText(seen), '$ echo hi\r\nhi\r\n$ ');
  await done();
});

test('an answer to a question goes to admind as an answer, apart from typing', async () => {
  const { fake, client, done } = await setup();
  const { panes: [pane] } = fake.addTerminal({ panes: 1 });
  watch(client, pane);
  await until(() => client.shown(pane));
  assert.equal(client.answer(pane, new TextEncoder().encode('\u001b[3;5R')), true);
  assert.equal(client.answer(pane, new TextEncoder().encode('ls\r')), false, 'what is not an escape sequence is no answer');
  await until(() => fake.answers.length === 1, 'the answer');
  assert.deepEqual(fake.answers, [{ pane, text: '\u001b[3;5R' }]);
  assert.equal(fake.log.some((message) => message.type === 'input'), false, 'an answer is not typing');
  await done();
});

test('typing more than 16 KiB is sent in frames of at most 16 KiB, in order', async () => {
  const { fake, client, done } = await setup();
  const { panes: [pane] } = fake.addTerminal({ panes: 1 });
  watch(client, pane);
  await until(() => client.shown(pane));
  const bytes = new Uint8Array(40000).fill(0x61);
  client.input(pane, bytes);
  await until(() => fake.log.filter((message) => message.type === 'input').length === 3, 'three frames');
  assert.deepEqual(fake.log.filter((message) => message.type === 'input').map((message) => message.bytes.length), [16384, 16384, 7232]);
  await done();
});

test('output is acknowledged soon after it is parsed, and at once past 16 KiB', async () => {
  const { fake, client, done } = await setup();
  const { panes: [pane] } = fake.addTerminal({ panes: 1 });
  const seen = watch(client, pane);
  await until(() => outputText(seen) === '$ ');
  await until(() => fake.acks.length === 1, 'the first ack');
  assert.deepEqual(fake.acks[0], { pane, offset: 2 });
  fake.write(pane, new Uint8Array(20000).fill(0x78));
  await until(() => fake.acks.length === 3, 'the others');
  assert.deepEqual(fake.acks.slice(1), [{ pane, offset: 16386 }, { pane, offset: 20002 }]);
  await done();
});

test('a fault is followed by new tries, and the pane is attached again with since: nothing lost, nothing twice', async () => {
  const { fake, client, told, done } = await setup();
  const { panes: [pane] } = fake.addTerminal({ panes: 1 });
  const seen = watch(client, pane);
  await until(() => outputText(seen) === '$ ');
  fake.write(pane, 'A');
  await until(() => outputText(seen) === '$ A');
  fake.drop(1006);
  await until(() => told.statuses.some((status) => status.state === 'reconnecting'), 'reconnecting');
  fake.write(pane, 'B');
  await until(() => told.statuses.filter((status) => status.state === 'open').length === 2, 'open again');
  await until(() => seen.attached.length === 2 && outputText(seen) === '$ AB', 'B');
  assert.equal(seen.attached[1].replay, false);
  assert.equal(seen.attached[1].offset, 3);
  assert.deepEqual(fake.log.filter((message) => message.type === 'attach').map((message) => message.since), [undefined, 3]);
  assert.deepEqual(told.statuses.find((status) => status.state === 'reconnecting'), { state: 'reconnecting', attempt: 1, refused: false });
  assert.equal(told.statuses.at(-1).attempt, 0, 'an open socket has no tries');
  await done();
});

test('a pane the page has a screen for is attached with since, and replayed when admind no longer has it', async () => {
  const { fake, client, done } = await setup();
  const { panes: [pane] } = fake.addTerminal({ panes: 1 });
  fake.write(pane, 'abc');
  const kept = watch(client, pane, { since: 5 });
  await until(() => kept.attached.length === 1 && outputText(kept) === '');
  assert.deepEqual([kept.attached[0].replay, kept.attached[0].offset], [false, 5]);
  const old = fake.addTerminal({ panes: 1 }).panes[0];
  const stale = watch(client, old, { since: 999 });
  await until(() => stale.attached.length === 1 && outputText(stale) === '$ ');
  assert.equal(stale.attached[0].replay, true);
  await done();
});

test('a refused handshake counts as a failed try, and is said to be one that was never open', async () => {
  const fake = await startFake();
  fake.refusals.push(403);
  const told = [];
  const client = openSocket({
    location: { protocol: 'http:', host: `127.0.0.1:${fake.port}` }, token: fake.token, onStatus: (status) => told.push(status), delay: () => 10,
  });
  await until(() => told.some((status) => status.state === 'open'), 'open');
  assert.deepEqual(told.map((status) => [status.state, status.attempt, status.refused]), [
    ['connecting', 0, undefined], ['reconnecting', 1, true], ['open', 0, undefined],
  ]);
  client.stop();
  await fake.close();
});

test('the tries wait as the schedule says, and the number counts up while they fail', async () => {
  const fake = await startFake();
  const waits = [];
  const told = [];
  fake.refusals.push(403, 403, 403);
  const client = openSocket({
    location: { protocol: 'http:', host: `127.0.0.1:${fake.port}` },
    token: fake.token,
    onStatus: (status) => told.push(status),
    setTimer: (fn, ms) => {
      waits.push(ms);
      return setTimeout(fn, 5);
    },
    clearTimer: clearTimeout,
  });
  await until(() => told.some((status) => status.state === 'open'), 'open');
  assert.deepEqual(waits.filter((ms) => ms >= 1000), [1000, 2000, 5000]);
  assert.deepEqual(told.filter((status) => status.state === 'reconnecting').map((status) => status.attempt), [1, 2, 3]);
  client.stop();
  await fake.close();
});

test('a session that ended is not tried again', async () => {
  const { fake, client, told, done } = await setup();
  await until(() => told.statuses.some((status) => status.state === 'open'));
  fake.drop(4001);
  await until(() => told.statuses.at(-1).state === 'ended', 'ended');
  await wait(60);
  assert.equal(fake.upgrades.length, 1);
  assert.equal(client.input(1, Uint8Array.of(1)), false);
  await done();
});

test('typing while the socket is not open is not sent', async () => {
  const { fake, client, done } = await setup({ delay: () => 60000, setTimer: () => 0 });
  const { panes: [pane] } = fake.addTerminal({ panes: 1 });
  watch(client, pane);
  await until(() => client.shown(pane));
  fake.drop(1006);
  await until(() => client.status.state === 'reconnecting');
  assert.equal(client.input(pane, Uint8Array.of(97)), false);
  assert.equal(client.input(7.5, Uint8Array.of(97)), false);
  await done();
});

test('a pane that ends, an error about a pane and a terminal that changed are told', async () => {
  const { fake, client, told, done } = await setup();
  const { id: terminal, panes: [pane] } = fake.addTerminal({ panes: 1 });
  const seen = watch(client, pane);
  const missing = watch(client, 9999);
  await until(() => client.shown(pane) && missing.errors.length === 1, 'the error');
  assert.equal(missing.errors[0].id, 'ADM-131');
  fake.changed(terminal);
  await until(() => told.changed.length === 1);
  assert.equal(told.changed[0], terminal);
  fake.endPane(pane, 'exited', 0);
  await until(() => seen.ended.length === 1);
  assert.deepEqual(seen.ended[0], { type: 'ended', pane, why: 'exited', status: 0 });
  assert.equal(client.shown(pane), false);
  await done();
});

test('a pane that admind stopped showing is told it, and may attach again', async () => {
  const { fake, client, done } = await setup();
  const { panes: [pane] } = fake.addTerminal({ panes: 1 });
  const seen = watch(client, pane);
  await until(() => client.shown(pane));
  fake.peers.values().next().value.text(encodeServer({ type: 'detached', pane, why: 'slow' }));
  await until(() => seen.detached.length === 1);
  assert.equal(client.shown(pane), false);
  await done();
});

test('a size asked for while the attach is on its way is sent once the pane is shown, and later at once', async () => {
  const { fake, client, done } = await setup();
  const { panes: [pane] } = fake.addTerminal({ panes: 1 });
  await until(() => client.status.state === 'open');
  watch(client, pane, { cols: 80, rows: 24 });
  client.resize(pane, 100, 30);
  await until(() => fake.log.some((message) => message.type === 'resize'), 'the resize');
  assert.deepEqual(fake.log.find((message) => message.type === 'resize'), { type: 'resize', pane, cols: 100, rows: 30 });
  client.resize(pane, 90, 20);
  await until(() => fake.log.filter((message) => message.type === 'resize').length === 2);
  assert.deepEqual([fake.pane(pane).cols, fake.pane(pane).rows], [90, 20]);
  client.resize(12345, 1, 1);
  await done();
});

test('detaching stops the pane being shown, and its output is not taken after', async () => {
  const { fake, client, done } = await setup();
  const { panes: [pane] } = fake.addTerminal({ panes: 1 });
  const seen = watch(client, pane);
  await until(() => outputText(seen) === '$ ');
  client.detach(pane);
  await until(() => fake.log.some((message) => message.type === 'detach'));
  fake.write(pane, 'later');
  await wait(40);
  assert.equal(outputText(seen), '$ ');
  client.detach(pane);
  assert.equal(fake.log.filter((message) => message.type === 'detach').length, 1, 'a pane that is not attached is not detached');
  await done();
});

test('output that overlaps what a pane has taken in is cut, and an empty piece is nothing', async () => {
  const { fake, client, done } = await setup();
  const { panes: [pane] } = fake.addTerminal({ panes: 1 });
  const seen = watch(client, pane);
  await until(() => outputText(seen) === '$ ');
  const peer = fake.peers.values().next().value;
  peer.binary(encodeServer({ type: 'output', pane, offset: 1, bytes: new TextEncoder().encode(' xy') }));
  peer.binary(encodeServer({ type: 'output', pane, offset: 0, bytes: new TextEncoder().encode('$ ') }));
  peer.binary(encodeServer({ type: 'output', pane: 777, offset: 0, bytes: new TextEncoder().encode('zz') }));
  await until(() => outputText(seen) === '$ xy', 'the cut piece');
  assert.deepEqual(seen.output.map((one) => one.offset), [0, 2]);
  await done();
});

test('a frame that cannot be read, and a handler that throws, are told and do not stop the socket', async () => {
  const { fake, client, told, done } = await setup();
  const { panes: [pane] } = fake.addTerminal({ panes: 1 });
  client.attach(pane, { cols: 80, rows: 24 }, { output: () => { throw new Error('boom'); } });
  await until(() => told.problems.length === 1, 'the handler failing');
  assert.match(told.problems[0], /output handler failed: boom/);
  fake.peers.values().next().value.text('not json');
  await until(() => told.problems.length === 2);
  assert.match(told.problems[1], /could not be read: not JSON/);
  fake.write(pane, 'x');
  await until(() => told.problems.length === 3, 'a third');
  await done();
});

test('an error that is about no pane the page shows is told as a problem', async () => {
  const { fake, told, done } = await setup();
  await until(() => told.statuses.some((status) => status.state === 'open'));
  fake.peers.values().next().value.text(encodeServer({ type: 'error', error: { id: 'ADM-130', title: 'Too many', detail: 'Sixteen panes.' }, pane: 4 }));
  await until(() => told.problems.length === 1);
  assert.equal(told.problems[0], 'Too many: Sixteen panes.');
  await done();
});

test('a try that is waiting can be made now', async () => {
  const waits = [];
  const { fake, client, told, done } = await setup({ setTimer: (fn, ms) => { waits.push([fn, ms]); return waits.length; }, clearTimer: () => {} });
  await until(() => told.statuses.some((status) => status.state === 'open'));
  client.retryNow();
  assert.equal(told.statuses.length, 2, 'nothing waits while open');
  fake.drop(1006);
  await until(() => client.status.state === 'reconnecting');
  client.retryNow();
  await until(() => client.status.state === 'open', 'open again');
  assert.equal(fake.upgrades.length, 2);
  await done();
});

test('a socket that is stopped does not try again, and a stopped one can be stopped again', async () => {
  const { fake, client, told, done } = await setup();
  await until(() => told.statuses.some((status) => status.state === 'open'));
  client.stop();
  await wait(60);
  assert.equal(fake.peers.size, 0);
  assert.equal(fake.upgrades.length, 1);
  client.retryNow();
  client.stop();
  await done();
});

test('a page with no token or no address has nothing to open', async () => {
  const problems = [];
  const told = [];
  const client = openSocket({ location: { protocol: 'http:', host: 'x' }, token: 'short', onStatus: (status) => told.push(status), onProblem: (words) => problems.push(words) });
  assert.deepEqual(told.map((status) => status.state), ['ended']);
  assert.equal(problems.length, 1);
  assert.equal(client.input(1, Uint8Array.of(1)), false);
  client.stop();
  const none = openSocket({ location: { protocol: 'file:', host: '' }, token: 'a'.repeat(20), onProblem: () => {} });
  assert.equal(none.status.state, 'ended');
});

test('a page of another origin is refused by the fake as the console refuses it', async () => {
  const fake = await startFake();
  const Impostor = class extends WebSocket {
    constructor(url, protocols) {
      super(url, { protocols, headers: { Origin: 'http://evil.example' } });
    }
  };
  const told = [];
  const client = openSocket({
    location: { protocol: 'http:', host: `127.0.0.1:${fake.port}` }, token: fake.token, Socket: Impostor, delay: () => 5000, onStatus: (status) => told.push(status),
  });
  await until(() => told.some((status) => status.state === 'reconnecting'), 'a refusal');
  assert.equal(told.some((status) => status.state === 'open'), false);
  assert.equal(told.at(-1).refused, true);
  client.stop();
  await fake.close();
});

test('messages about a pane the page does not show are let go', async () => {
  const { fake, told, done } = await setup();
  await until(() => told.statuses.some((status) => status.state === 'open'));
  const peer = fake.peers.values().next().value;
  peer.text(encodeServer({ type: 'attached', pane: 5, terminal: 1, name: 'x', admin: false, cols: 80, rows: 24, offset: 0, live: 0, replay: true }));
  peer.text(encodeServer({ type: 'ended', pane: 5, why: 'exited' }));
  peer.text(encodeServer({ type: 'detached', pane: 5, why: 'slow' }));
  peer.text(encodeServer({ type: 'changed', terminal: 5 }));
  await until(() => told.changed.length === 1, 'the last of them');
  assert.deepEqual(told.problems, []);
  await done();
});

test('a handler that throws something that is not an error, and a size no pane has, are told or not sent', async () => {
  const { fake, client, told, done } = await setup();
  const { panes: [pane] } = fake.addTerminal({ panes: 1 });
  client.attach(pane, { cols: 80, rows: 24 }, { attached: () => { throw 'plain words'; } });
  await until(() => told.problems.length === 1);
  assert.match(told.problems[0], /attached handler failed: plain words/);
  client.attach(pane + 1, { cols: 1, rows: 24 });
  await wait(30);
  assert.equal(fake.log.filter((message) => message.type === 'attach').length, 1, 'a size no pane has is not asked for');
  await done();
});

test('a pane that ends with all its output acknowledged sends no second ack', async () => {
  const { fake, client, done } = await setup();
  const { panes: [pane] } = fake.addTerminal({ panes: 1 });
  const seen = watch(client, pane);
  await until(() => fake.acks.length === 1);
  fake.endPane(pane, 'ended');
  await until(() => seen.ended.length === 1);
  await wait(80);
  assert.equal(fake.acks.length, 1);
  await done();
});
