// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The two directions of the terminal's socket that the page does not use:
// what admind sends, encoded, and what the page sends, decoded. The tests
// and the fake server (term-fake-server.mjs) stand in for admind with them,
// on the same tables and checks as the page's own two directions.

import { internals } from '../pages/terminal/term/protocol.js';

const { CLIENT, SERVER, fromJson, toJson, frame, parse, bytesOf, guarded } = internals;
const MAX_CHUNK = 16 * 1024;

const isObject = (value) => typeof value === 'object' && value !== null && !Array.isArray(value);

/** What admind sends for `message`: a string for the JSON messages, bytes for 'output', or null when it does not fit. */
export function encodeServer(message) {
  return guarded(() => {
    if (isObject(message) && message.type === 'output') return frame(message.pane, message.offset, message.bytes, true);
    return toJson(SERVER, message);
  }, null);
}

/** The message in a frame the page sent, as `{ ok: true, message }` or `{ ok: false, reason }`. */
export function decodeClient(data) {
  return guarded(() => readClient(data), { ok: false, reason: 'not a frame' });
}

function readClient(data) {
  if (typeof data === 'string') {
    const parsed = parse(data);
    return parsed.ok ? fromJson(CLIENT, parsed.raw) : parsed;
  }
  const bytes = bytesOf(data);
  if (bytes === null) return { ok: false, reason: 'not a frame' };
  if (bytes.length < 5) return { ok: false, reason: 'short frame' };
  if (bytes.length > 4 + MAX_CHUNK) return { ok: false, reason: 'too long' };
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  return { ok: true, message: { type: 'input', pane: view.getUint32(0), bytes: bytes.subarray(4) } };
}
