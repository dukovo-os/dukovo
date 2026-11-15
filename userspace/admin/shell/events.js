// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The session's event stream, `GET /api/1/events`: unlock, session, task,
// change and trial; it also tells admind that a tab is open. The browser
// opens a lost stream again itself and gives up only on an answer that is no
// stream, such as a session that is gone; `onFailure` hears each failure and
// whether it gave up.

const NAMES = ['unlock', 'session', 'task', 'change', 'trial'];

// Two failures of the stream within this long mean the server is not answering.
const WINDOW_MS = 10_000;

/** Watches the stream's failures: two within ten seconds are a server that does not answer. */
export function makeStreamWatch() {
  let last = null;
  return {
    /** The stream failed at `now`. Gives true when that is the second time within the window. */
    failed(now) {
      const second = last !== null && now - last <= WINDOW_MS;
      last = now;
      return second;
    },
    /** The stream opened. */
    opened() {
      last = null;
    },
  };
}

/** Follows the stream. Gives `{close}`. */
export function followEvents({ onEvent, onFailure, onOpen }) {
  const source = new EventSource('/api/1/events');
  for (const name of NAMES) {
    source.addEventListener(name, (event) => {
      let data = null;
      try {
        data = JSON.parse(event.data);
      } catch {
        return;
      }
      onEvent(name, data);
    });
  }
  source.addEventListener('open', () => onOpen?.());
  source.addEventListener('error', () => onFailure(source.readyState === EventSource.CLOSED));
  return { close: () => source.close() };
}
