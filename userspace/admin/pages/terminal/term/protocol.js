// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The messages of the terminal's WebSocket (design 13.7), as plain values:
// encoded to what the socket sends, and decoded from what it receives, with
// no socket and no DOM, so a test reaches all of it.
//
// A message is an object with a `type`. Text frames are JSON that has
// exactly one key naming the type; binary frames carry bytes after a
// header of big-endian numbers.
//
// From the page to admind:
//
//   { type: 'attach', pane, cols, rows, since? }    since: the offset after the last byte seen
//   { type: 'detach', pane }
//   { type: 'resize', pane, cols, rows }
//   { type: 'ack', pane, offset }                   the offset after the last byte parsed
//   { type: 'input', pane, bytes }                  binary: [u32 pane][bytes]
//   { type: 'answer', pane, text }                  the screen's answer to a question the program asked
//
// From admind to the page:
//
//   { type: 'attached', pane, terminal, name, admin, cols, rows, offset, live, replay, modes? }
//                                                   live: the offset of the first byte written after the
//                                                   attach; the screen answers no question before it.
//                                                   modes: on a replay, the escape sequences that put a
//                                                   new screen in the pane's modes, read before its bytes
//   { type: 'ended', pane, why, status? }            status: left out when admind does not know it
//   { type: 'detached', pane, why }
//   { type: 'changed', terminal }
//   { type: 'error', error: { id, title, detail, fix? }, pane? }   other keys of the error are dropped
//   { type: 'output', pane, offset, bytes }         binary: [u32 pane][u64 offset][bytes]
//
// `encodeClient` and `decodeServer` are the page's two directions. The
// others, for the tests and for anything that stands in for admind, are in
// tests/term-server-wire.mjs, built on `internals`.
//
// Nothing here throws. An encoder gives null for a message that does not
// fit, and a decoder gives `{ ok: false, reason }`, or `{ ok: true,
// message }`, so that one bad frame never stops the page. Unknown keys in
// a JSON message are ignored, but the type must be one, and exactly one,
// of the known ones.

/** The WebSocket subprotocol the console speaks. */
export const SUBPROTOCOL = 'dukovo-terminal-1';
/** The route of the socket. */
export const SOCKET_PATH = '/api/1/terminal';

/** The most bytes a binary frame carries after its header, in either direction. */
export const MAX_CHUNK = 16 * 1024;
/** The most a text message can be (the frame limit of 13.7), in characters: more is refused unread. Every field is bounded, so a message the page encodes is far below it. */
export const MAX_MESSAGE = 64 * 1024;

const MAX_ID = 0xffffffff;
const MAX_NAME = 200;
const MAX_ERROR_ID = 32;
const MAX_ERROR_TITLE = 200;
const MAX_ERROR_DETAIL = 2000;
const MAX_STATUS = 0xffff;
const MAX_MODES = 1024;
const MAX_ANSWER = 64;
const ENDED_WHY = ['exited', 'ended', 'locked', 'signed-out', 'idle', 'restart'];

// What each field may be. A validator says whether a value is acceptable.
const whole = (low, high) => (value) => Number.isSafeInteger(value) && value >= low && value <= high;
const text = (low, high) => (value) => typeof value === 'string' && value.length >= low && value.length <= high && !/[\p{Cc}]/u.test(value);
const oneOf = (words) => (value) => words.includes(value);
const flag = (value) => typeof value === 'boolean';
// The modes of a replay: escape sequences, so control characters are what it holds.
const sequences = (value) => typeof value === 'string' && value.length <= MAX_MODES;
// An answer to a question: an escape sequence of printable ASCII after its ESC.
const answerText = (value) => typeof value === 'string' && value.length <= MAX_ANSWER && /^\x1b[\x20-\x7e]+$/.test(value);

const isObject = (value) => typeof value === 'object' && value !== null && !Array.isArray(value);
const has = (object, key) => Object.hasOwn(object, key);

// An error as the server gives it: `id` one word, `title` and `detail` text of a bounded size.
const ERROR_LIMITS = { id: MAX_ERROR_ID, title: MAX_ERROR_TITLE, detail: MAX_ERROR_DETAIL };
const isError = (value) =>
  isObject(value) &&
  Object.entries(ERROR_LIMITS).every(([name, limit]) => typeof value[name] === 'string' && value[name].length <= limit) &&
  /^[\w-]+$/.test(value.id);

// The error without the keys the page does not use (`field`, `command`); `fix` stays when it is text.
const cleanError = ({ id, title, detail, fix }) => ({
  id, title, detail, ...(typeof fix === 'string' && fix.length <= MAX_ERROR_DETAIL ? { fix } : {}),
});

const PANE = whole(0, MAX_ID);
const COLS = whole(2, 500);
const ROWS = whole(1, 300);
const OFFSET = whole(0, Number.MAX_SAFE_INTEGER);

// The JSON messages by type. The type is the key of the message, and its
// value is `id`, the number the message is about (or, for an error, the
// error), which the decoded message holds under that name. `fields` are
// the other keys, each with its validator, and `optional` the ones that
// may be left out.
const CLIENT = {
  attach: { id: 'pane', check: PANE, fields: { cols: COLS, rows: ROWS, since: OFFSET }, optional: ['since'] },
  detach: { id: 'pane', check: PANE, fields: {}, optional: [] },
  resize: { id: 'pane', check: PANE, fields: { cols: COLS, rows: ROWS }, optional: [] },
  ack: { id: 'pane', check: PANE, fields: { offset: OFFSET }, optional: [] },
  answer: { id: 'pane', check: PANE, fields: { text: answerText }, optional: [] },
};
const SERVER = {
  attached: {
    id: 'pane',
    check: PANE,
    fields: { terminal: OFFSET, name: text(1, MAX_NAME), admin: flag, cols: COLS, rows: ROWS, offset: OFFSET, live: OFFSET, replay: flag, modes: sequences },
    optional: ['modes'],
  },
  ended: { id: 'pane', check: PANE, fields: { why: oneOf(ENDED_WHY), status: whole(0, MAX_STATUS) }, optional: ['status'] },
  detached: { id: 'pane', check: PANE, fields: { why: oneOf(['slow']) }, optional: [] },
  changed: { id: 'terminal', check: OFFSET, fields: {}, optional: [] },
  error: { id: 'error', check: isError, fields: { pane: PANE }, optional: ['pane'] },
};

// The message made of a decoded JSON `raw`, by the table `types`, or the
// reason it is not one.
function fromJson(types, raw) {
  if (!isObject(raw)) return { ok: false, reason: 'not an object' };
  const present = Object.keys(types).filter((type) => has(raw, type));
  if (present.length === 0) return { ok: false, reason: 'unknown type' };
  if (present.length > 1) return { ok: false, reason: 'more than one type' };
  const [type] = present;
  const message = { type, [types[type].id]: raw[type] };
  for (const name of Object.keys(types[type].fields)) {
    if (has(raw, name)) message[name] = raw[name];
  }
  return checked(types, message);
}

// The message with only its own fields if every one fits, or why not.
function checked(types, message) {
  if (!has(types, message.type)) return { ok: false, reason: 'unknown type' };
  const spec = types[message.type];
  const { type } = message;
  const value = message[spec.id];
  if (!spec.check(value)) return { ok: false, reason: `${type} has a bad ${spec.id}` };
  const clean = { type, [spec.id]: type === 'error' ? cleanError(value) : value };
  for (const [name, valid] of Object.entries(spec.fields)) {
    if (message[name] === undefined && spec.optional.includes(name)) continue;
    if (message[name] === undefined) return { ok: false, reason: `${type} lacks ${name}` };
    if (!valid(message[name])) return { ok: false, reason: `${type} has a bad ${name}` };
    clean[name] = message[name];
  }
  return { ok: true, message: clean };
}

// The JSON text of a message that fits, with its type's key first, or null.
function toJson(types, message) {
  if (!isObject(message)) return null;
  const result = checked(types, message);
  if (!result.ok) return null;
  const { type, ...rest } = result.message;
  const { [types[type].id]: value, ...others } = rest;
  return JSON.stringify({ [type]: value, ...others });
}

// Parses the text of a frame.
function parse(data) {
  if (data.length > MAX_MESSAGE) return { ok: false, reason: 'too long' };
  try {
    return { ok: true, raw: JSON.parse(data) };
  } catch {
    return { ok: false, reason: 'not JSON' };
  }
}

// The bytes of a binary frame as a Uint8Array, or null for what is not one.
function bytesOf(data) {
  if (data instanceof Uint8Array) return data;
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  return null;
}

// A message that carries bytes: its header numbers, then the bytes.
function frame(pane, offset, bytes, withOffset) {
  if (!PANE(pane) || !(bytes instanceof Uint8Array) || bytes.length > MAX_CHUNK) return null;
  if (withOffset && !OFFSET(offset)) return null;
  const head = withOffset ? 12 : 4;
  const out = new Uint8Array(head + bytes.length);
  const view = new DataView(out.buffer);
  view.setUint32(0, pane);
  if (withOffset) view.setBigUint64(4, BigInt(offset));
  out.set(bytes, head);
  return out;
}

// Gives what `work` gives, or `fallback` when it throws: a message object
// whose getters throw, or a frame that is not what it claims, must not stop
// the page.
function guarded(work, fallback) {
  try {
    return work();
  } catch {
    return fallback;
  }
}

/**
 * What the page sends for `message`, or null when it does not fit: a
 * string for the JSON messages, a Uint8Array for 'input' (at most MAX_CHUNK
 * bytes, and at least one; `inputFrames` cuts more).
 */
export function encodeClient(message) {
  return guarded(() => {
    if (isObject(message) && message.type === 'input') {
      return message.bytes?.length > 0 ? frame(message.pane, 0, message.bytes, false) : null;
    }
    return toJson(CLIENT, message);
  }, null);
}

/**
 * The messages for typing `bytes` into `pane`, as binary frames of at most
 * MAX_CHUNK bytes each, in order, or null when `pane` is not a pane number.
 */
export function inputFrames(pane, bytes) {
  if (!PANE(pane) || !(bytes instanceof Uint8Array)) return null;
  const frames = [];
  for (let at = 0; at < bytes.length; at += MAX_CHUNK) frames.push(frame(pane, 0, bytes.subarray(at, at + MAX_CHUNK), false));
  return frames;
}

/**
 * The message in a frame admind sent (a string, or binary as an ArrayBuffer
 * or a view), as `{ ok: true, message }`, or `{ ok: false, reason }`. The
 * bytes of an 'output' message are a view into the frame, not a copy.
 */
export function decodeServer(data) {
  return guarded(() => readServer(data), { ok: false, reason: 'not a frame' });
}

function readServer(data) {
  if (typeof data === 'string') {
    const parsed = parse(data);
    return parsed.ok ? fromJson(SERVER, parsed.raw) : parsed;
  }
  const bytes = bytesOf(data);
  if (bytes === null) return { ok: false, reason: 'not a frame' };
  if (bytes.length < 12) return { ok: false, reason: 'short frame' };
  if (bytes.length > 12 + MAX_CHUNK) return { ok: false, reason: 'too long' };
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const offset = view.getBigUint64(4);
  if (offset > BigInt(Number.MAX_SAFE_INTEGER)) return { ok: false, reason: 'output has a bad offset' };
  return { ok: true, message: { type: 'output', pane: view.getUint32(0), offset: Number(offset), bytes: bytes.subarray(12) } };
}

/** The offset after the last byte of an 'output' message: what the page acknowledges. */
export function endOffset(message) {
  return message.offset + message.bytes.length;
}

const TOKEN = /^[A-Za-z0-9_-]{16,128}$/;

/**
 * The subprotocols to offer the WebSocket constructor: the console's, and
 * the page's CSRF token, the one header a browser lets a page set. Null when
 * the token is not what admind makes (base64url).
 */
export function socketProtocols(token) {
  return typeof token === 'string' && TOKEN.test(token) ? [SUBPROTOCOL, `csrf.${token}`] : null;
}

/**
 * The address of the socket for a page at `location` (its `protocol` and
 * `host`): wss for https and ws for http. Null for any other page.
 */
export function socketUrl(location) {
  if (!isObject(location) || typeof location.host !== 'string' || !/^[\w.:[\]-]+$/.test(location.host)) return null;
  if (location.protocol === 'https:') return `wss://${location.host}${SOCKET_PATH}`;
  return location.protocol === 'http:' ? `ws://${location.host}${SOCKET_PATH}` : null;
}

/** What tests/term-server-wire.mjs builds admind's two directions from. */
export const internals = { CLIENT, SERVER, fromJson, toJson, frame, parse, bytesOf, guarded };
