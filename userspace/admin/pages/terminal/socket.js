// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The page's side of the terminal's socket (design 13.7): one WebSocket for
// all the panes it shows, tried again after 1, 2, 5 and 10 seconds and then
// every 30, each pane attached again with `since` (the offset after the
// last byte it took in) so that nothing is lost or shown twice, and the
// output acknowledged once the page has parsed it, which is what lets
// admind hold a flood back. It has no DOM: the WebSocket class and the
// timers are given, so the tests run it against a fake admind.
//
// A pane is attached with handlers, each optional:
//   attached(message)      shown; a replay (message.replay) starts a new screen,
//                          and output before message.live was written before
//   output(bytes, offset)  output to parse, in order, once
//   ended(message)         the pane ended
//   detached(message)      admind stopped showing it (the page took its output too slowly)
//   error(error)           what was asked of it could not be done

import {
  decodeServer, encodeClient, endOffset, inputFrames, socketProtocols, socketUrl,
} from './term/protocol.js';
import { delayFor, retries } from './reconnect-model.js';

// Output is acknowledged when this much has been parsed, or soon after.
const ACK_BYTES = 16 * 1024;
const ACK_MS = 50;
const OPEN = 1;

const nothing = () => {};

/**
 * Opens the socket for a page at `location` with the session's CSRF
 * `token`. `onStatus({ state, attempt, refused })` is called as the state
 * changes: 'connecting', 'open', 'reconnecting' (try number `attempt`,
 * `refused` when no socket was ever opened: a browser hides why) or 'ended'
 * (the session ended: no more tries). `onChanged(terminal)` hears that a
 * terminal changed elsewhere. `onProblem(words)` hears what the page cannot
 * use (a frame it cannot read, a handler that threw).
 */
export function openSocket({
  location, token, onStatus = nothing, onChanged = nothing, onProblem = nothing,
  Socket = globalThis.WebSocket, setTimer = setTimeout, clearTimer = clearTimeout, delay = delayFor,
}) {
  const url = socketUrl(location);
  const protocols = socketProtocols(token);
  const panes = new Map();
  let ws = null;
  let state = 'connecting';
  let attempt = 0;
  let timer = null;
  let stopped = false;
  let opened = false;

  const tell = (extra = {}) => onStatus({ state, attempt, ...extra });
  const send = (message) => {
    const data = encodeClient(message);
    if (data === null || ws?.readyState !== OPEN) return false;
    ws.send(data);
    return true;
  };
  const call = (entry, name, ...args) => {
    try {
      entry.handlers[name]?.(...args);
    } catch (error) {
      onProblem(`A pane's ${name} handler failed: ${error?.message ?? error}`);
    }
  };

  function ask(pane, entry) {
    entry.shown = false;
    entry.asked = { cols: entry.cols, rows: entry.rows };
    send({ type: 'attach', pane, cols: entry.cols, rows: entry.rows, ...(entry.offset === null ? {} : { since: entry.offset }) });
  }

  function ack(pane, entry) {
    clearTimer(entry.timer);
    entry.timer = null;
    if (entry.unacked === 0) return;
    entry.unacked = 0;
    send({ type: 'ack', pane, offset: entry.offset });
  }

  function output(message) {
    const entry = panes.get(message.pane);
    if (!entry) return;
    let { bytes } = message;
    let { offset } = message;
    // What a pane has taken in is not given again.
    if (entry.offset !== null && offset < entry.offset) {
      bytes = bytes.subarray(Math.min(entry.offset - offset, bytes.length));
      offset = entry.offset;
    }
    if (bytes.length === 0) return;
    call(entry, 'output', bytes, offset);
    entry.offset = endOffset({ offset, bytes });
    entry.unacked += bytes.length;
    if (entry.unacked >= ACK_BYTES) ack(message.pane, entry);
    else if (entry.timer === null) entry.timer = setTimer(() => ack(message.pane, entry), ACK_MS);
  }

  function received(message) {
    const entry = panes.get(message.pane);
    switch (message.type) {
      case 'output':
        output(message);
        break;
      case 'attached':
        if (!entry) break;
        entry.shown = true;
        entry.offset = message.offset;
        entry.unacked = 0;
        call(entry, 'attached', message);
        // A size asked for while the attach was on its way.
        if (entry.cols !== entry.asked.cols || entry.rows !== entry.asked.rows) {
          send({ type: 'resize', pane: message.pane, cols: entry.cols, rows: entry.rows });
        }
        break;
      case 'ended':
        if (!entry) break;
        entry.shown = false;
        ack(message.pane, entry);
        call(entry, 'ended', message);
        break;
      case 'detached':
        if (!entry) break;
        entry.shown = false;
        call(entry, 'detached', message);
        break;
      case 'changed':
        onChanged(message.terminal);
        break;
      default:
        if (entry) call(entry, 'error', message.error);
        else onProblem(`${message.error.title}: ${message.error.detail}`);
    }
  }

  function closed(event) {
    ws = null;
    for (const entry of panes.values()) {
      entry.shown = false;
      clearTimer(entry.timer);
      entry.timer = null;
      entry.unacked = 0;
    }
    const refused = !opened;
    opened = false;
    if (!retries(event?.code)) {
      state = 'ended';
      tell();
      return;
    }
    attempt += 1;
    state = 'reconnecting';
    tell({ refused });
    timer = setTimer(open, delay(attempt));
  }

  function open() {
    timer = null;
    if (url === null || protocols === null) {
      state = 'ended';
      onProblem('The page has no address or token for the terminal\'s socket.');
      tell();
      return;
    }
    const mine = new Socket(url, protocols);
    ws = mine;
    // A refused handshake gives an error and, in some browsers, no close: the first of the two ends the try.
    let over = false;
    const finish = (event) => {
      if (over || ws !== mine) return;
      over = true;
      closed(event);
    };
    ws.binaryType = 'arraybuffer';
    ws.onopen = () => {
      opened = true;
      attempt = 0;
      state = 'open';
      tell();
      for (const [pane, entry] of panes) ask(pane, entry);
    };
    ws.onmessage = (event) => {
      const read = decodeServer(event.data);
      if (read.ok) received(read.message);
      else onProblem(`A frame from the console could not be read: ${read.reason}`);
    };
    ws.onclose = finish;
    ws.onerror = () => {
      if (!opened) finish({ code: 1006 });
    };
    // A try after a failure goes on saying "try 3"; only the first says it is connecting.
    if (state === 'connecting') tell();
  }

  open();

  return {
    /** The state now: `{ state, attempt }`. */
    get status() {
      return { state, attempt };
    },

    /** Whether pane `pane` is shown on the open socket. */
    shown: (pane) => Boolean(panes.get(pane)?.shown),

    /**
     * Shows pane `pane`, `cols` by `rows`, with `handlers`. `since` is the
     * offset after the last byte of its output that the page has parsed, when
     * it has a screen with it; without it, the pane is replayed.
     */
    attach(pane, { cols, rows, since = null }, handlers = {}) {
      const entry = {
        cols, rows, offset: since, handlers, shown: false, asked: { cols, rows }, unacked: 0, timer: null,
      };
      panes.set(pane, entry);
      if (state === 'open') ask(pane, entry);
    },

    /** Stops showing a pane. */
    detach(pane) {
      const entry = panes.get(pane);
      if (!entry) return;
      ack(pane, entry);
      panes.delete(pane);
      send({ type: 'detach', pane });
    },

    /** Gives a pane another size, when it is shown, and when it is, as soon as it is. */
    resize(pane, cols, rows) {
      const entry = panes.get(pane);
      if (!entry) return;
      Object.assign(entry, { cols, rows });
      if (entry.shown) send({ type: 'resize', pane, cols, rows });
    },

    /** Types `bytes` in a pane. False when the socket is not open, and nothing is sent. */
    input(pane, bytes) {
      const frames = state === 'open' && ws?.readyState === OPEN ? inputFrames(pane, bytes) : null;
      if (frames === null) return false;
      for (const frame of frames) ws.send(frame);
      return true;
    },

    /**
     * Sends the screen's answer to a question that pane `pane`'s program
     * asked, as `bytes`. admind writes it to the program from one page
     * alone, so that one question has one answer. False when the socket is
     * not open, or the answer is not one.
     */
    answer(pane, bytes) {
      return send({ type: 'answer', pane, text: String.fromCharCode(...bytes) });
    },

    /** Tries now, when the next try is waiting (the network came back, the tab was shown). */
    retryNow() {
      if (timer === null || stopped) return;
      clearTimer(timer);
      open();
    },

    /** Closes the socket for good. */
    stop() {
      stopped = true;
      clearTimer(timer);
      for (const entry of panes.values()) clearTimer(entry.timer);
      if (ws && ws.readyState <= OPEN) ws.close(1000);
      ws = null;
    },
  };
}
