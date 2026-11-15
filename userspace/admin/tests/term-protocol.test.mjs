// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  MAX_CHUNK,
  MAX_MESSAGE,
  SOCKET_PATH,
  SUBPROTOCOL,
  decodeServer,
  encodeClient,
  endOffset,
  inputFrames,
  socketProtocols,
  socketUrl,
} from '../pages/terminal/term/protocol.js';
import { decodeClient, encodeServer } from './term-server-wire.mjs';

const attached = {
  type: 'attached', pane: 12, terminal: 3, name: 'alice@duke-nas', admin: false, cols: 120, rows: 40, offset: 65536, live: 98304, replay: true,
};
const error = { id: 'ADM-130', title: 'Too many terminals', detail: 'You have 32 terminals.' };

// Messages with their JSON, for the page's direction and for admind's.
const CLIENT = [
  [{ type: 'attach', pane: 12, cols: 120, rows: 40, since: 81920 }, '{"attach":12,"cols":120,"rows":40,"since":81920}'],
  [{ type: 'attach', pane: 12, cols: 120, rows: 40 }, '{"attach":12,"cols":120,"rows":40}'],
  [{ type: 'detach', pane: 12 }, '{"detach":12}'],
  [{ type: 'resize', pane: 12, cols: 2, rows: 300 }, '{"resize":12,"cols":2,"rows":300}'],
  [{ type: 'ack', pane: 12, offset: 90112 }, '{"ack":12,"offset":90112}'],
  [{ type: 'answer', pane: 12, text: '\u001b[24;1R' }, '{"answer":12,"text":"\\u001b[24;1R"}'],
];
const SERVER = [
  [attached, '{"attached":12,"terminal":3,"name":"alice@duke-nas","admin":false,"cols":120,"rows":40,"offset":65536,"live":98304,"replay":true}'],
  [
    { ...attached, modes: '\u001b[?1049h\u001b[?25l' },
    '{"attached":12,"terminal":3,"name":"alice@duke-nas","admin":false,"cols":120,"rows":40,"offset":65536,"live":98304,"replay":true,"modes":"\\u001b[?1049h\\u001b[?25l"}',
  ],
  [{ type: 'ended', pane: 12, why: 'exited', status: 0 }, '{"ended":12,"why":"exited","status":0}'],
  [{ type: 'ended', pane: 12, why: 'locked' }, '{"ended":12,"why":"locked"}'],
  [{ type: 'detached', pane: 12, why: 'slow' }, '{"detached":12,"why":"slow"}'],
  [{ type: 'changed', terminal: 3 }, '{"changed":3}'],
  [{ type: 'error', error }, '{"error":{"id":"ADM-130","title":"Too many terminals","detail":"You have 32 terminals."}}'],
  [{ type: 'error', error, pane: 12 }, '{"error":{"id":"ADM-130","title":"Too many terminals","detail":"You have 32 terminals."},"pane":12}'],
  [
    { type: 'error', error: { ...error, fix: 'End one on the Sessions page.' }, pane: 12 },
    '{"error":{"id":"ADM-130","title":"Too many terminals","detail":"You have 32 terminals.","fix":"End one on the Sessions page."},"pane":12}',
  ],
];

test('every message the page sends is encoded and decoded', () => {
  for (const [message, json] of CLIENT) {
    assert.equal(encodeClient(message), json);
    assert.deepEqual(decodeClient(json), { ok: true, message });
  }
});

test('every message admind sends is encoded and decoded', () => {
  for (const [message, json] of SERVER) {
    assert.equal(encodeServer(message), json);
    assert.deepEqual(decodeServer(json), { ok: true, message });
  }
});

test('every kind of ended', () => {
  for (const why of ['exited', 'ended', 'locked', 'signed-out', 'idle', 'restart']) {
    const message = { type: 'ended', pane: 1, why, status: 3 };
    assert.deepEqual(decodeServer(encodeServer(message)), { ok: true, message });
  }
});

test('typing is a binary frame: the pane, then the bytes', () => {
  const frame = encodeClient({ type: 'input', pane: 0x01020304, bytes: Uint8Array.of(0x61, 0x1b, 0x5b, 0x41) });
  assert.deepEqual([...frame], [1, 2, 3, 4, 0x61, 0x1b, 0x5b, 0x41]);
  const back = decodeClient(frame);
  assert.equal(back.ok, true);
  assert.equal(back.message.type, 'input');
  assert.equal(back.message.pane, 0x01020304);
  assert.deepEqual([...back.message.bytes], [0x61, 0x1b, 0x5b, 0x41]);
  assert.deepEqual(decodeClient(frame.buffer).message.pane, 0x01020304);
});

test('output is a binary frame: the pane, the offset, then the bytes', () => {
  const bytes = Uint8Array.of(104, 105);
  const frame = encodeServer({ type: 'output', pane: 12, offset: 0x0102030405, bytes });
  assert.deepEqual([...frame], [0, 0, 0, 12, 0, 0, 0, 1, 2, 3, 4, 5, 104, 105]);
  const back = decodeServer(frame);
  assert.deepEqual([back.ok, back.message.type, back.message.pane, back.message.offset], [true, 'output', 12, 0x0102030405]);
  assert.deepEqual([...back.message.bytes], [104, 105]);
  assert.equal(endOffset(back.message), 0x0102030405 + 2);
});

test('an offset can be as large as a number holds exactly', () => {
  const offset = Number.MAX_SAFE_INTEGER;
  const frame = encodeServer({ type: 'output', pane: 1, offset, bytes: Uint8Array.of(1) });
  assert.equal(decodeServer(frame).message.offset, offset);
  assert.equal(encodeServer({ type: 'output', pane: 1, offset: offset + 1, bytes: Uint8Array.of(1) }), null);
  const wild = new Uint8Array(13);
  new DataView(wild.buffer).setBigUint64(4, 2n ** 53n);
  assert.deepEqual(decodeServer(wild), { ok: false, reason: 'output has a bad offset' });
  new DataView(wild.buffer).setBigUint64(4, 2n ** 64n - 1n);
  assert.equal(decodeServer(wild).ok, false);
});

test('a frame may be an ArrayBuffer or a view into one', () => {
  const frame = encodeServer({ type: 'output', pane: 7, offset: 9, bytes: Uint8Array.of(1, 2, 3) });
  assert.deepEqual(decodeServer(frame.buffer), decodeServer(frame));
  const padded = new Uint8Array(frame.length + 10);
  padded.set(frame, 5);
  const view = padded.subarray(5, 5 + frame.length);
  assert.deepEqual([...decodeServer(view).message.bytes], [1, 2, 3]);
  const data = new DataView(padded.buffer, 5, frame.length);
  assert.equal(decodeServer(data).message.pane, 7);
  assert.equal(decodeServer(data).message.offset, 9);
});

test('a frame can carry up to 16 KiB, and not a byte more', () => {
  assert.equal(MAX_CHUNK, 16384);
  const full = new Uint8Array(MAX_CHUNK).fill(65);
  assert.equal(encodeClient({ type: 'input', pane: 1, bytes: full }).length, 4 + MAX_CHUNK);
  assert.equal(encodeServer({ type: 'output', pane: 1, offset: 0, bytes: full }).length, 12 + MAX_CHUNK);
  const over = new Uint8Array(MAX_CHUNK + 1);
  assert.equal(encodeClient({ type: 'input', pane: 1, bytes: over }), null);
  assert.equal(encodeServer({ type: 'output', pane: 1, offset: 0, bytes: over }), null);
  assert.equal(decodeClient(new Uint8Array(4 + MAX_CHUNK + 1)).ok, false);
  assert.equal(decodeServer(new Uint8Array(12 + MAX_CHUNK + 1)).ok, false);
  assert.equal(decodeClient(new Uint8Array(4 + MAX_CHUNK)).ok, true);
  assert.equal(decodeServer(new Uint8Array(12 + MAX_CHUNK)).ok, true);
});

test('long input is cut into frames in order', () => {
  const bytes = Uint8Array.from({ length: 40000 }, (_, i) => i % 251);
  const frames = inputFrames(5, bytes);
  assert.equal(frames.length, 3);
  assert.deepEqual(frames.map((f) => f.length - 4), [16384, 16384, 7232]);
  const joined = frames.flatMap((frame) => [...decodeClient(frame).message.bytes]);
  assert.deepEqual(joined, [...bytes]);
  assert.equal(decodeClient(frames[2]).message.pane, 5);
  assert.equal(inputFrames(5, new Uint8Array(MAX_CHUNK)).length, 1);
  assert.equal(inputFrames(5, new Uint8Array(MAX_CHUNK + 1)).length, 2);
  assert.deepEqual(inputFrames(5, new Uint8Array(0)), []);
  assert.equal(inputFrames(-1, bytes), null);
  assert.equal(inputFrames(1.5, bytes), null);
  assert.equal(inputFrames(5, 'text'), null);
});

test('input with nothing in it is not sent', () => {
  assert.equal(encodeClient({ type: 'input', pane: 1, bytes: new Uint8Array(0) }), null);
  assert.equal(encodeClient({ type: 'input', pane: 1 }), null);
  assert.equal(encodeClient({ type: 'input', pane: 1, bytes: 'abc' }), null);
  assert.equal(encodeClient({ type: 'input', pane: 1, bytes: [1, 2] }), null);
  assert.equal(encodeClient({ type: 'input', bytes: Uint8Array.of(1) }), null);
  assert.equal(encodeClient({ type: 'input', pane: 2 ** 32, bytes: Uint8Array.of(1) }), null);
});

test('the empty output frame is allowed and short frames are not', () => {
  assert.equal(decodeServer(new Uint8Array(12)).message.bytes.length, 0);
  assert.deepEqual(decodeServer(new Uint8Array(11)), { ok: false, reason: 'short frame' });
  assert.deepEqual(decodeServer(new Uint8Array(0)), { ok: false, reason: 'short frame' });
  assert.deepEqual(decodeClient(new Uint8Array(4)), { ok: false, reason: 'short frame' });
  assert.equal(decodeClient(new Uint8Array(5)).ok, true);
  assert.equal(encodeServer({ type: 'output', pane: 1, offset: -1, bytes: new Uint8Array(0) }), null);
  assert.equal(encodeServer({ type: 'output', pane: 1, offset: 1.5, bytes: new Uint8Array(0) }), null);
  assert.equal(encodeServer({ type: 'output', pane: 1, bytes: new Uint8Array(0) }), null);
  assert.equal(encodeServer({ type: 'output', pane: 1, offset: 0, bytes: 'x' }), null);
});

test('what is not text or bytes is refused', () => {
  for (const data of [undefined, null, 5, {}, [], new Blob(['x']), true]) {
    assert.deepEqual(decodeServer(data), { ok: false, reason: 'not a frame' });
    assert.deepEqual(decodeClient(data), { ok: false, reason: 'not a frame' });
  }
});

test('text that is no message is refused', () => {
  for (const text of ['', 'x', '{', '[1]', 'null', '5', '"attach"', 'true', '{"attach":12', '[]']) {
    const server = decodeServer(text);
    const client = decodeClient(text);
    assert.equal(server.ok, false, text);
    assert.equal(client.ok, false, text);
    assert.equal(typeof server.reason, 'string');
  }
  assert.deepEqual(decodeServer('x'), { ok: false, reason: 'not JSON' });
  assert.deepEqual(decodeServer('[1]'), { ok: false, reason: 'not an object' });
  assert.deepEqual(decodeServer('null'), { ok: false, reason: 'not an object' });
});

test('an unknown type is refused, and a message of the other direction too', () => {
  assert.deepEqual(decodeServer('{}'), { ok: false, reason: 'unknown type' });
  assert.deepEqual(decodeServer('{"hello":1}'), { ok: false, reason: 'unknown type' });
  assert.deepEqual(decodeServer('{"pane":1,"cols":2}'), { ok: false, reason: 'unknown type' });
  assert.equal(decodeServer('{"attach":12,"cols":80,"rows":24}').ok, false);
  assert.equal(decodeClient('{"attached":12}').ok, false);
  assert.equal(decodeClient('{"ended":1,"why":"exited"}').ok, false);
  for (const key of ['constructor', 'toString', '__proto__', 'hasOwnProperty']) {
    assert.equal(decodeServer(`{"${key}":1}`).ok, false, key);
    assert.equal(decodeClient(`{"${key}":1}`).ok, false, key);
    assert.equal(encodeClient({ type: key, pane: 1 }), null, key);
    assert.equal(encodeServer({ type: key, pane: 1 }), null, key);
  }
});

test('a message of two types is refused', () => {
  assert.deepEqual(decodeClient('{"attach":1,"detach":1,"cols":80,"rows":24}'), { ok: false, reason: 'more than one type' });
  assert.deepEqual(decodeServer('{"ended":1,"detached":1,"why":"slow"}'), { ok: false, reason: 'more than one type' });
  assert.equal(decodeServer('{"changed":1,"error":{"id":"X","title":"t","detail":"d"}}').ok, false);
});

test('a message with a field missing is refused', () => {
  assert.deepEqual(decodeClient('{"attach":12,"cols":80}'), { ok: false, reason: 'attach lacks rows' });
  assert.deepEqual(decodeClient('{"attach":12,"rows":24}'), { ok: false, reason: 'attach lacks cols' });
  assert.deepEqual(decodeClient('{"resize":12,"cols":80}'), { ok: false, reason: 'resize lacks rows' });
  assert.deepEqual(decodeClient('{"ack":12}'), { ok: false, reason: 'ack lacks offset' });
  for (const name of ['terminal', 'name', 'admin', 'cols', 'rows', 'offset', 'replay']) {
    const raw = JSON.parse(SERVER[0][1]);
    delete raw[name];
    assert.deepEqual(decodeServer(JSON.stringify(raw)), { ok: false, reason: `attached lacks ${name}` });
  }
  assert.deepEqual(decodeServer('{"ended":12}'), { ok: false, reason: 'ended lacks why' });
  assert.deepEqual(decodeServer('{"detached":12}'), { ok: false, reason: 'detached lacks why' });
  assert.equal(decodeServer('{"error":{"id":"X","title":"t"}}').ok, false);
  assert.equal(decodeServer('{"error":{"id":"X","detail":"d"}}').ok, false);
  assert.equal(decodeServer('{"error":{"title":"t","detail":"d"}}').ok, false);
  assert.equal(encodeClient({ type: 'attach', pane: 1, cols: 80 }), null);
  assert.equal(encodeClient({ type: 'resize', pane: 1, rows: 24 }), null);
  assert.equal(encodeServer({ type: 'ended', pane: 1 }), null);
});

test('a number out of its range is refused', () => {
  const bad = (text) => assert.equal(decodeClient(text).ok, false, text);
  bad('{"attach":-1,"cols":80,"rows":24}');
  bad('{"attach":4294967296,"cols":80,"rows":24}');
  bad('{"attach":1.5,"cols":80,"rows":24}');
  bad('{"attach":"1","cols":80,"rows":24}');
  bad('{"attach":null,"cols":80,"rows":24}');
  bad('{"attach":1,"cols":1,"rows":24}');
  bad('{"attach":1,"cols":501,"rows":24}');
  bad('{"attach":1,"cols":80,"rows":0}');
  bad('{"attach":1,"cols":80,"rows":301}');
  bad('{"attach":1,"cols":"80","rows":24}');
  bad('{"attach":1,"cols":80.5,"rows":24}');
  bad('{"attach":1,"cols":80,"rows":24,"since":-1}');
  bad('{"attach":1,"cols":80,"rows":24,"since":1.5}');
  bad('{"attach":1,"cols":80,"rows":24,"since":null}');
  bad('{"attach":1,"cols":80,"rows":24,"since":9007199254740993}');
  bad('{"resize":1,"cols":501,"rows":24}');
  bad('{"resize":1,"cols":80,"rows":-5}');
  bad('{"ack":1,"offset":-1}');
  bad('{"ack":1,"offset":1e400}');
  bad('{"ack":1,"offset":"5"}');
  bad('{"detach":[1]}');
  bad('{"detach":true}');
  assert.equal(decodeClient('{"attach":4294967295,"cols":500,"rows":300,"since":9007199254740991}').ok, true);
  assert.equal(decodeClient('{"attach":0,"cols":2,"rows":1,"since":0}').ok, true);
  assert.equal(encodeClient({ type: 'attach', pane: 1, cols: 501, rows: 24 }), null);
  assert.equal(encodeClient({ type: 'attach', pane: 1, cols: 80, rows: 24, since: -1 }), null);
  assert.equal(encodeClient({ type: 'resize', pane: 1, cols: 1, rows: 24 }), null);
  assert.equal(encodeClient({ type: 'resize', pane: 1, cols: 80, rows: 301 }), null);
  assert.equal(encodeClient({ type: 'ack', pane: 1, offset: NaN }), null);
  assert.equal(encodeClient({ type: 'ack', pane: 1, offset: Infinity }), null);
  assert.equal(encodeClient({ type: 'ack', pane: 1, offset: 2 ** 53 }), null);
  assert.equal(encodeClient({ type: 'detach', pane: 2 ** 32 }), null);
  assert.equal(encodeClient({ type: 'detach', pane: '1' }), null);
});

test('the fields of admind\'s messages are checked as well', () => {
  const change = (name, value) => JSON.stringify({ ...JSON.parse(SERVER[0][1]), [name]: value });
  for (const [name, value] of [
    ['terminal', -1], ['terminal', '3'], ['name', ''], ['name', 5], ['name', 'a\nb'], ['name', 'a\x1bb'], ['name', 'x'.repeat(201)],
    ['admin', 0], ['admin', 'false'], ['cols', 1], ['cols', 501], ['rows', 0], ['rows', 301], ['offset', -1], ['offset', 1.5],
    ['replay', null], ['replay', 1], ['live', -1], ['live', '5'], ['live', null],
  ]) {
    assert.equal(decodeServer(change(name, value)).ok, false, `${name} ${JSON.stringify(value)}`);
  }
  assert.equal(decodeServer(change('name', 'x'.repeat(200))).ok, true);
  assert.equal(decodeServer('{"ended":1,"why":"gone"}').ok, false);
  assert.equal(decodeServer('{"ended":1,"why":5}').ok, false);
  assert.equal(decodeServer('{"ended":1,"why":"exited","status":-1}').ok, false);
  assert.equal(decodeServer('{"ended":1,"why":"exited","status":65536}').ok, false);
  assert.equal(decodeServer('{"ended":1,"why":"exited","status":1.5}').ok, false);
  assert.equal(decodeServer('{"ended":1,"why":"exited","status":65535}').ok, true);
  assert.equal(decodeServer('{"detached":1,"why":"fast"}').ok, false);
  assert.equal(decodeServer('{"changed":-1}').ok, false);
  assert.equal(decodeServer('{"changed":"3"}').ok, false);
  assert.equal(decodeServer('{"ended":-1,"why":"exited"}').ok, false);
});

test('an error has three texts of a bounded size, and may name a pane', () => {
  const wrap = (value, pane) => JSON.stringify(pane === undefined ? { error: value } : { error: value, pane });
  for (const value of [
    null, 5, 'x', [], {}, { id: 1, title: 't', detail: 'd' }, { id: 'X', title: 5, detail: 'd' }, { id: 'X', title: 't', detail: null },
    { id: 'two words', title: 't', detail: 'd' }, { id: '', title: 't', detail: 'd' }, { id: 'X'.repeat(33), title: 't', detail: 'd' },
    { id: 'X', title: 't'.repeat(201), detail: 'd' }, { id: 'X', title: 't', detail: 'd'.repeat(2001) },
  ]) {
    assert.equal(decodeServer(wrap(value)).ok, false, wrap(value));
  }
  assert.equal(decodeServer(wrap({ id: 'X', title: 't'.repeat(200), detail: 'd'.repeat(2000) })).ok, true);
  assert.equal(decodeServer(wrap({ id: 'ADM-1', title: 't', detail: 'a\nb' })).ok, true);
  assert.equal(decodeServer(wrap(error, -1)).ok, false);
  assert.equal(decodeServer(wrap(error, '1')).ok, false);
  assert.equal(decodeServer(wrap(error, null)).ok, false);
  assert.equal(encodeServer({ type: 'error', error: { id: 'X', title: 't' } }), null);
  assert.equal(encodeServer({ type: 'error', error: 'text' }), null);
  assert.equal(encodeServer({ type: 'error', error, pane: -1 }), null);
});

test('keys the message does not know are ignored, and the message is rebuilt clean', () => {
  const result = decodeServer('{"detached":4,"why":"slow","extra":{"deep":[1]},"__proto__":{"x":1}}');
  assert.deepEqual(result, { ok: true, message: { type: 'detached', pane: 4, why: 'slow' } });
  assert.equal(Object.getPrototypeOf(result.message), Object.prototype);
  assert.equal(result.message.x, undefined);
  const withError = decodeServer('{"error":{"id":"X","title":"t","detail":"d","more":1},"pane":2}');
  assert.deepEqual(withError.message.error, { id: 'X', title: 't', detail: 'd' });
  // admind's whole error body: the keys the page does not use go, `fix` stays
  const whole = decodeServer('{"error":{"id":"ADM-125","title":"t","detail":"d","fix":"f","field":"cols","command":{"words":["x"]}},"pane":2}');
  assert.deepEqual(whole.message.error, { id: 'ADM-125', title: 't', detail: 'd', fix: 'f' });
  for (const fix of [7, null, 'f'.repeat(2001)]) {
    const odd = decodeServer(JSON.stringify({ error: { id: 'X', title: 't', detail: 'd', fix } }));
    assert.deepEqual(odd.message.error, { id: 'X', title: 't', detail: 'd' }, `a fix of ${typeof fix}`);
  }
  assert.equal(encodeClient({ type: 'detach', pane: 1, extra: 1 }), '{"detach":1}');
});

test('a message may be at most 64 KiB', () => {
  const room = MAX_MESSAGE - '{"ack":1,"offset":1,"x":""}'.length;
  assert.equal(decodeClient(`{"ack":1,"offset":1,"x":"${'a'.repeat(room)}"}`).ok, true);
  assert.deepEqual(decodeClient(`{"ack":1,"offset":1,"x":"${'a'.repeat(MAX_MESSAGE)}"}`), { ok: false, reason: 'too long' });
  assert.deepEqual(decodeServer(' '.repeat(MAX_MESSAGE + 1)), { ok: false, reason: 'too long' });
});

test('encoders refuse what is not a message', () => {
  for (const value of [undefined, null, 5, 'attach', [], {}, { type: 5 }, { type: 'nothing' }, { pane: 1 }]) {
    assert.equal(encodeClient(value), null);
    assert.equal(encodeServer(value), null);
  }
  assert.equal(encodeClient({ type: 'attached', pane: 1 }), null);
  assert.equal(encodeServer({ type: 'attach', pane: 1, cols: 80, rows: 24 }), null);
});

test('nothing throws, whatever it is given', () => {
  const trap = { get type() { throw new Error('type'); } };
  const input = { type: 'input', get pane() { throw new Error('pane'); }, bytes: Uint8Array.of(1) };
  const proxy = new Proxy({}, { getPrototypeOf() { throw new Error('proxy'); } });
  assert.equal(encodeClient(trap), null);
  assert.equal(encodeClient(input), null);
  assert.equal(encodeServer(trap), null);
  assert.equal(encodeServer({ type: 'output', get pane() { throw new Error('x'); } }), null);
  assert.equal(decodeServer(proxy).ok, false);
  assert.equal(decodeClient(proxy).ok, false);
  assert.equal(socketUrl(proxy), null);
  assert.equal(socketProtocols({ toString() { throw new Error('t'); } }), null);
});

test('a message that was decoded encodes to the same message', () => {
  // A seeded stream of mostly valid values that are slightly off.
  let seed = 7;
  const next = (n) => {
    seed = (Math.imul(seed, 1103515245) + 12345) >>> 0;
    return seed % n;
  };
  const values = [0, 1, 2, 12, 80, 300, 500, 501, -1, 1.5, 65535, 65536, 2 ** 32, 2 ** 53, null, true, false, '', 'x', 'exited', 'slow', [], {}];
  const keys = ['attach', 'detach', 'resize', 'ack', 'attached', 'ended', 'detached', 'changed', 'error', 'pane', 'terminal', 'name',
    'admin', 'cols', 'rows', 'since', 'offset', 'replay', 'why', 'status', 'id', 'title', 'detail'];
  let accepted = 0;
  for (let i = 0; i < 20000; i++) {
    const raw = {};
    for (let k = next(7); k > 0; k--) raw[keys[next(keys.length)]] = values[next(values.length)];
    const text = JSON.stringify(raw);
    for (const [decode, encode] of [[decodeClient, encodeClient], [decodeServer, encodeServer]]) {
      const result = decode(text);
      if (!result.ok) {
        assert.equal(typeof result.reason, 'string');
        continue;
      }
      accepted += 1;
      const again = decode(encode(result.message));
      assert.deepEqual(again, result);
    }
  }
  assert.ok(accepted > 0);
});

test('random bytes never make a decoder throw', () => {
  let seed = 11;
  const next = (n) => {
    seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
    return seed % n;
  };
  for (let i = 0; i < 2000; i++) {
    const bytes = Uint8Array.from({ length: next(40) }, () => next(256));
    for (const result of [decodeServer(bytes), decodeClient(bytes), decodeServer(String.fromCharCode(...bytes))]) {
      assert.equal(typeof result.ok, 'boolean');
    }
  }
});

test('the subprotocols carry the token', () => {
  assert.equal(SUBPROTOCOL, 'dukovo-terminal-1');
  const token = 'A'.repeat(20) + '-_0z'.repeat(6);
  assert.deepEqual(socketProtocols(token), ['dukovo-terminal-1', `csrf.${token}`]);
  assert.deepEqual(socketProtocols('abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG'), [
    'dukovo-terminal-1',
    'csrf.abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG',
  ]);
  for (const bad of [undefined, null, 5, '', 'short', 'x'.repeat(129), 'has space 0123456789abcdef', 'dots.0123456789abcdef', 'comma,0123456789abcdef', 'new\nline0123456789abcdef', 'unicode\u00e9012345678901234']) {
    assert.equal(socketProtocols(bad), null, String(bad));
  }
});

test('the socket address follows the page\'s', () => {
  assert.equal(SOCKET_PATH, '/api/1/terminal');
  assert.equal(socketUrl({ protocol: 'https:', host: 'duke-nas:8443' }), 'wss://duke-nas:8443/api/1/terminal');
  assert.equal(socketUrl({ protocol: 'http:', host: '192.168.1.2:8080' }), 'ws://192.168.1.2:8080/api/1/terminal');
  assert.equal(socketUrl({ protocol: 'https:', host: '[fe80::1]:8443' }), 'wss://[fe80::1]:8443/api/1/terminal');
  for (const bad of [undefined, null, 'https://x', {}, { protocol: 'https:' }, { protocol: 'https:', host: '' }, { protocol: 'ftp:', host: 'x' },
    { protocol: '__proto__', host: 'x' }, { protocol: 'https:', host: 'x/evil' }, { protocol: 'https:', host: 'a b' }, { protocol: 'https:', host: 5 }]) {
    assert.equal(socketUrl(bad), null, JSON.stringify(bad));
  }
});

test('an answer is an escape sequence of printable characters, of 64 at most', () => {
  const answer = (text) => encodeClient({ type: 'answer', pane: 1, text });
  for (const good of ['\u001b[5;10R', '\u001b[0n', '\u001b[?62;22c', `\u001b${'1'.repeat(63)}`]) {
    assert.notEqual(answer(good), null, JSON.stringify(good));
  }
  for (const bad of ['', '\u001b', '[5;10R', 'ls\r', '\u001b[5;10R\r', '\u001b[5;\u001b10R', `\u001b${'1'.repeat(64)}`, 5, null]) {
    assert.equal(answer(bad), null, JSON.stringify(bad));
  }
  assert.equal(decodeServer('{"attached":1,"terminal":1,"name":"a","admin":false,"cols":80,"rows":24,"offset":0,"replay":true}').ok, false, 'live is not optional');
});

test('the modes of a replay are kept: escape sequences, at most 1024 characters, left out when there is no replay', () => {
  const modes = '\u001b[?1h\u001b[?7h\u001b[?1049h\u001b[?1002h\u001b[?1006h\u001b[?2004h';
  const replay = decodeServer(JSON.stringify({ ...JSON.parse(encodeServer(attached)), modes }));
  assert.equal(replay.ok, true);
  assert.equal(replay.message.modes, modes);
  const plain = decodeServer(encodeServer({ ...attached, replay: false }));
  assert.equal(Object.hasOwn(plain.message, 'modes'), false);
  const raw = JSON.parse(encodeServer(attached));
  assert.equal(decodeServer(JSON.stringify({ ...raw, modes: '' })).message.modes, '');
  assert.equal(decodeServer(JSON.stringify({ ...raw, modes: 'x'.repeat(1024) })).ok, true);
  assert.deepEqual(decodeServer(JSON.stringify({ ...raw, modes: 'x'.repeat(1025) })), { ok: false, reason: 'attached has a bad modes' });
  for (const odd of [1, null, true, ['x'], { a: 1 }]) {
    assert.deepEqual(decodeServer(JSON.stringify({ ...raw, modes: odd })), { ok: false, reason: 'attached has a bad modes' });
  }
});
