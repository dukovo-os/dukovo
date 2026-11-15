// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Following the log (design 5.5 and 23.5): the client of `GET
// /api/1/logs/follow`, a stream of server-sent events, and what keeps it
// going for a page. The stream is read with `fetch`, so that a refusal's own
// words (a ninth follow, a locked console) can be read, and so that nothing
// ever opens it again by itself. A follow runs only while the tab is seen:
// a tab that stays hidden for a minute closes it, and when the tab is shown
// again the page reads the lines it missed and follows again. Nothing here
// touches the page, so each part is tested in Node.

import { ApiError } from '../../api.js';
import { BROKEN_WORDS, endedWords, followEvent } from '../../lib/log-model.js';

/** How long a tab may stay hidden before the follow is closed: a quick look at another tab does not end it. */
export const HIDDEN_MS = 60_000;

/**
 * Reads server-sent events from text that comes in pieces: `push(text)` calls
 * `onEvent({type, data})` for each event that is complete so far. A comment
 * is skipped, a line break is `\n`, `\r\n` or `\r`, and an event that the
 * stream ends in the middle of is dropped.
 */
export function makeEventReader(onEvent) {
  let buffer = '';
  let type = '';
  let data = [];

  function line(text) {
    if (text === '') {
      if (data.length > 0) onEvent({ type: type || 'message', data: data.join('\n') });
      type = '';
      data = [];
      return;
    }
    if (text.startsWith(':')) return;
    const colon = text.indexOf(':');
    const name = colon < 0 ? text : text.slice(0, colon);
    const value = colon < 0 ? '' : text.slice(colon + 1).replace(/^ /, '');
    if (name === 'event') type = value;
    else if (name === 'data') data.push(value);
  }

  return {
    push(text) {
      buffer += text;
      let start = 0;
      for (let at = 0; at < buffer.length; at += 1) {
        const mark = buffer[at];
        if (mark !== '\n' && mark !== '\r') continue;
        // A carriage return at the very end may be the first half of `\r\n`: it waits for the next piece.
        if (mark === '\r' && at === buffer.length - 1) break;
        line(buffer.slice(start, at));
        if (mark === '\r' && buffer[at + 1] === '\n') at += 1;
        start = at + 1;
      }
      buffer = buffer.slice(start);
    },
  };
}

/** The error for an answer that says nothing the console can read. */
const unreadable = (rest) => new ApiError({
  title: 'The console got an answer it cannot read',
  detail: `The server answered with status ${rest.status} and no explanation.`,
  fix: 'Try again. If it keeps happening, look at the server\'s log.',
  ...rest,
});

/** The error of an answer that is no stream: admind's own words, with the wait a 429 asks for. */
async function refusal(response) {
  let body = null;
  try {
    body = await response.json();
  } catch {
    // An answer that is not JSON is said in general words below.
  }
  const retryAfter = Number.parseInt(response.headers.get('Retry-After'), 10);
  const rest = { status: response.status, retryAfter: Number.isFinite(retryAfter) ? retryAfter : null };
  return body?.error?.title ? new ApiError({ ...body.error, ...rest }) : unreadable(rest);
}

/**
 * Opens the follow at `path` and reads it to its end, calling `on(event)` for
 * each event of `followEvent`'s kinds; an event that does not read is
 * skipped. Gives a promise of how it ended: `{reason: 'ended', ended}` when
 * the stream said why, `{reason: 'refused', error}` when it was refused
 * before it began, `{reason: 'broken'}` when the connection went with no
 * word, and `{reason: 'aborted'}` when `signal` closed it. `fetcher` is
 * `fetch`; a test gives its own.
 */
export async function openFollow({
  path, signal, on, fetcher = (...given) => fetch(...given),
}) {
  let response;
  try {
    response = await fetcher(path, { headers: { Accept: 'text/event-stream' }, credentials: 'same-origin', cache: 'no-store', signal });
  } catch {
    return { reason: signal?.aborted ? 'aborted' : 'broken' };
  }
  if (!response.ok) return { reason: 'refused', error: await refusal(response) };
  if (!(response.headers.get('Content-Type') ?? '').startsWith('text/event-stream') || !response.body) {
    return { reason: 'refused', error: unreadable({ status: response.status, retryAfter: null }) };
  }

  let ended = null;
  const events = makeEventReader(({ type, data }) => {
    if (ended) return;
    const event = followEvent(type, data);
    if (!event) return;
    if (event.kind === 'ended') ended = event;
    on(event);
  });
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      events.push(decoder.decode(value, { stream: true }));
      if (ended) break;
    }
  } catch {
    return { reason: signal?.aborted ? 'aborted' : 'broken' };
  } finally {
    reader.cancel().catch(() => {});
  }
  if (ended) return { reason: 'ended', ended };
  return { reason: signal?.aborted ? 'aborted' : 'broken' };
}

/**
 * What keeps a follow going for a page. `open(signal, on)` starts a read of
 * the stream (`openFollow` with the page's path); `readGap()` reads and adds
 * the lines that came since the last one the list has; `onEvent(event)`
 * hears the `command`, `record` and `skipped` events; `onState(state,
 * detail)` hears each change: `'on'`, `'off'`, `'away'` (the tab has been
 * hidden for a minute, and the follow waits for it to be seen) and
 * `'stopped'`, with `{words, error}` for why. `visible()` says whether the tab
 * is seen; `later` and `cancel` are `setTimeout` and `clearTimeout`.
 */
export function makeFollower({
  open, readGap, onEvent, onState, visible, later = (work, wait) => setTimeout(work, wait), cancel = (timer) => clearTimeout(timer),
}) {
  let state = 'off';
  let controller = null;
  let grace = null;

  const set = (next, detail = {}) => {
    state = next;
    onState(next, detail);
  };

  function endOfRun(run, result) {
    if (controller !== run) return;
    controller = null;
    if (result.reason === 'aborted') return;
    if (result.reason === 'ended') set('stopped', { words: endedWords(result.ended), why: result.ended.why });
    else if (result.reason === 'refused') set('stopped', { words: 'Following could not start.', error: result.error });
    else set('stopped', { words: BROKEN_WORDS });
  }

  function begin() {
    const run = new AbortController();
    controller = run;
    set('on');
    open(run.signal, onEvent).then((result) => endOfRun(run, result), () => endOfRun(run, { reason: 'broken' }));
  }

  function end() {
    if (grace !== null) cancel(grace);
    grace = null;
    const run = controller;
    controller = null;
    run?.abort();
  }

  const follower = {
    /** The state now. */
    state: () => state,

    /** Begins following; a follow that is going keeps going. */
    start() {
      if (state === 'on') return;
      end();
      begin();
      if (!visible()) follower.hidden();
    },

    /** The person's Pause. */
    stop() {
      end();
      if (state !== 'off') set('off');
    },

    /** Closes the stream and opens another, as when the whole log becomes readable. */
    restart() {
      if (state !== 'on') return;
      end();
      begin();
      if (!visible()) follower.hidden();
    },

    /** The tab is hidden: the follow ends if it stays so for a minute. */
    hidden() {
      if (state !== 'on' || grace !== null) return;
      grace = later(() => {
        grace = null;
        end();
        set('away');
      }, HIDDEN_MS);
    },

    /** The tab is seen: a follow that waits goes on, and the lines it missed are read. */
    shown() {
      if (grace !== null) {
        cancel(grace);
        grace = null;
      }
      if (state !== 'away') return;
      begin();
      Promise.resolve(readGap()).catch(() => {});
    },

    /** The page is left: nothing is left open. */
    dispose() {
      end();
      state = 'off';
    },
  };
  return follower;
}
