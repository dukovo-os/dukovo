// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The follow client of the Logs page (design 5.5 and 23.5) against a fake
// stream: events read from text that arrives in pieces, an event that is bad
// skipped, each way the stream can end or be refused, and what keeps a
// follow going while the tab is hidden and shown.

import assert from 'node:assert/strict';
import test from 'node:test';

import { ApiError } from '../api.js';
import {
  HIDDEN_MS, makeEventReader, makeFollower, openFollow,
} from '../pages/logs/follow.js';

const encoder = new TextEncoder();
const record = (seq, over = {}) => ({
  time: '2026-11-14T09:30:00.123456Z', boot: 'b1', seq, level: 'warning', from: 'service', name: 'nginx', process: 312, user: 0, text: `line ${seq}`, ...over,
});
/** The text of one event of the stream. */
const sse = (type, value) => `event: ${type}\ndata: ${JSON.stringify(value)}\n\n`;

/** The events a reader gives for `chunks`. */
function readAll(chunks) {
  const found = [];
  const reader = makeEventReader((event) => found.push(event));
  for (const chunk of chunks) reader.push(chunk);
  return found;
}

/** A fetcher that answers with a stream of `chunks`, and then closes it (or leaves it open until the request is aborted). */
function streamOf(chunks, { open = false, status = 200, type = 'text/event-stream', headers = {} } = {}) {
  const calls = [];
  const fetcher = async (path, options) => {
    calls.push({ path, options });
    const body = new ReadableStream({
      start(controller) {
        for (const chunk of chunks) controller.enqueue(typeof chunk === 'string' ? encoder.encode(chunk) : chunk);
        if (!open) controller.close();
        options.signal?.addEventListener('abort', () => {
          try {
            controller.error(new DOMException('aborted', 'AbortError'));
          } catch {
            // Already closed.
          }
        });
      },
    });
    return new Response(body, { status, headers: { 'Content-Type': type, ...headers } });
  };
  return { fetcher, calls };
}

/** A fetcher that answers with a refusal: JSON, or other words. */
const refusedWith = (status, body, headers = {}) => async () => new Response(typeof body === 'string' ? body : JSON.stringify(body), {
  status, headers: { 'Content-Type': typeof body === 'string' ? 'text/plain' : 'application/json', ...headers },
});

// ---- the events of a stream -------------------------------------------------------------------------

test('events are read from lines, with their type and their data', () => {
  assert.deepEqual(readAll(['event: skipped\ndata: {"count":3}\n\nevent: ended\ndata: {"why":"time"}\n\n']), [
    { type: 'skipped', data: '{"count":3}' },
    { type: 'ended', data: '{"why":"time"}' },
  ]);
});

test('an event with no type is a message, and a space after the colon is optional', () => {
  assert.deepEqual(readAll(['data:one\n\ndata: two\n\ndata:  three\n\n']), [
    { type: 'message', data: 'one' }, { type: 'message', data: 'two' }, { type: 'message', data: ' three' },
  ]);
});

test('lines of data are joined with a line break, and a comment is skipped', () => {
  assert.deepEqual(readAll([': open\n\nevent: x\ndata: a\n: a comment in the middle\ndata: b\n\n']), [{ type: 'x', data: 'a\nb' }]);
});

test('an event with no data is dropped, and does not leave its type for the next', () => {
  assert.deepEqual(readAll(['event: lonely\n\ndata: x\n\n']), [{ type: 'message', data: 'x' }]);
});

test('a line break may be a line feed, a carriage return or both', () => {
  const expected = [{ type: 'a', data: '1' }, { type: 'b', data: '2' }];
  assert.deepEqual(readAll(['event: a\ndata: 1\n\nevent: b\ndata: 2\n\n']), expected);
  assert.deepEqual(readAll(['event: a\r\ndata: 1\r\n\r\nevent: b\r\ndata: 2\r\n\r\n']), expected);
  // A carriage return at the end of a piece waits to see whether a line feed follows.
  assert.deepEqual(readAll(['event: a\rdata: 1\r\revent: b\rdata: 2\r\r: end\n']), expected);
  assert.deepEqual(readAll(['event: a\rdata: 1\r\revent: b\rdata: 2\r\r']), [expected[0]]);
});

test('an event is the same however the stream is cut into pieces', () => {
  const text = `: open\n\n${sse('command', { shown: 'elevate log' })}${sse('record', record(1, { text: 'až' }))}event: x\r\ndata: 1\r\ndata: 2\r\n\r\nevent: y\rdata: 3\r\r: end\n`;
  const whole = readAll([text]);
  assert.equal(whole.length, 4);
  for (let cut = 1; cut < text.length; cut += 1) assert.deepEqual(readAll([text.slice(0, cut), text.slice(cut)]), whole, `cut at ${cut}`);
  assert.deepEqual(readAll([...text]), whole, 'one character at a time');
});

test('an event the stream ends in the middle of is not given', () => {
  assert.deepEqual(readAll(['event: a\ndata: 1\n\nevent: b\ndata: 2\n']), [{ type: 'a', data: '1' }]);
  assert.deepEqual(readAll(['data: 1']), []);
});

// ---- the stream -----------------------------------------------------------------------------------------------

test('the stream is asked for as event-stream, with the session\'s cookie, and its events come in order until it ends', async () => {
  const { fetcher, calls } = streamOf([
    sse('command', { shown: 'elevate log --quiet --output json --follow --lines 0 --priority warning' }),
    sse('record', record(1)), sse('record', record(2)), sse('skipped', { count: 40 }), sse('record', record(3)),
    sse('ended', { why: 'locked' }),
  ]);
  const heard = [];
  const controller = new AbortController();
  const result = await openFollow({ path: '/api/1/logs/follow?priority=warning', signal: controller.signal, on: (event) => heard.push(event), fetcher });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].path, '/api/1/logs/follow?priority=warning');
  assert.equal(calls[0].options.headers.Accept, 'text/event-stream');
  assert.equal(calls[0].options.credentials, 'same-origin');
  assert.equal(calls[0].options.signal, controller.signal);
  assert.deepEqual(heard.map((event) => event.kind), ['command', 'record', 'record', 'skipped', 'record', 'ended']);
  assert.equal(heard[0].shown, 'elevate log --quiet --output json --follow --lines 0 --priority warning');
  assert.deepEqual(heard.filter((event) => event.kind === 'record').map((event) => event.record.seq), [1, 2, 3]);
  assert.equal(heard[3].count, 40);
  assert.equal(result.reason, 'ended');
  assert.equal(result.ended.why, 'locked');
});

test('an event that is bad is skipped, and the ones after it come', async () => {
  const { fetcher } = streamOf([
    sse('record', record(1)), 'event: record\ndata: {"seq":\n\n', sse('record', record(2, { seq: 'x' })), sse('mystery', { a: 1 }), sse('record', record(4)), sse('ended', { why: 'time' }),
  ]);
  const heard = [];
  await openFollow({ path: '/p', on: (event) => heard.push(event), fetcher });
  assert.deepEqual(heard.filter((event) => event.kind === 'record').map((event) => event.record.seq), [1, 4]);
});

test('a record whose text breaks across two pieces, and a letter cut in two, are read whole', async () => {
  const bytes = encoder.encode(sse('record', record(1, { text: 'příliš žluťoučký kůň' })));
  const middle = bytes.indexOf(0xc5) + 1;
  const { fetcher } = streamOf([bytes.slice(0, middle), bytes.slice(middle, middle + 7), bytes.slice(middle + 7), sse('ended', { why: 'time' })]);
  const heard = [];
  await openFollow({ path: '/p', on: (event) => heard.push(event), fetcher });
  assert.equal(heard[0].record.text, 'příliš žluťoučký kůň');
});

test('what comes after the end is not read', async () => {
  const { fetcher } = streamOf([`${sse('ended', { why: 'slow' })}${sse('record', record(9))}`]);
  const heard = [];
  const result = await openFollow({ path: '/p', on: (event) => heard.push(event), fetcher });
  assert.deepEqual(heard.map((event) => event.kind), ['ended']);
  assert.equal(result.reason, 'ended');
});

test('a stream that closes with no word is a broken connection', async () => {
  const { fetcher } = streamOf([sse('record', record(1))]);
  const heard = [];
  const result = await openFollow({ path: '/p', on: (event) => heard.push(event), fetcher });
  assert.equal(heard.length, 1);
  assert.deepEqual(result, { reason: 'broken' });
});

test('a fetch that fails is a broken connection, and one that was aborted is aborted', async () => {
  const failing = async () => { throw new TypeError('network error'); };
  assert.deepEqual(await openFollow({ path: '/p', on() {}, fetcher: failing }), { reason: 'broken' });
  const controller = new AbortController();
  controller.abort();
  const aborting = async (path, options) => {
    if (options.signal.aborted) throw new DOMException('aborted', 'AbortError');
    return null;
  };
  assert.deepEqual(await openFollow({ path: '/p', signal: controller.signal, on() {}, fetcher: aborting }), { reason: 'aborted' });
});

test('closing the request while the stream is open ends it as aborted, and nothing is given after', async () => {
  const { fetcher } = streamOf([sse('record', record(1))], { open: true });
  const controller = new AbortController();
  const heard = [];
  const done = openFollow({ path: '/p', signal: controller.signal, on: (event) => heard.push(event), fetcher });
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(heard.length, 1);
  controller.abort();
  assert.deepEqual(await done, { reason: 'aborted' });
  assert.equal(heard.length, 1);
});

test('a stream that is refused says why, in admind\'s words, with the wait it asks for', async () => {
  const many = { error: { id: 'ADM-134', title: 'The console follows as many logs as it may', detail: 'Eight are being followed.', fix: 'Try again in a moment.' } };
  const result = await openFollow({ path: '/p', on() {}, fetcher: refusedWith(429, many, { 'Retry-After': '30' }) });
  assert.equal(result.reason, 'refused');
  assert.ok(result.error instanceof ApiError);
  assert.equal(result.error.id, 'ADM-134');
  assert.equal(result.error.status, 429);
  assert.equal(result.error.retryAfter, 30);
  assert.equal(result.error.title, 'The console follows as many logs as it may');
  assert.equal(result.error.fix, 'Try again in a moment.');
});

test('each refusal before the stream begins is an error with its ID', async () => {
  for (const [status, id] of [[403, 'ADM-106'], [403, 'ADM-122'], [403, 'ADM-109'], [400, 'ADM-108'], [400, 'ADM-125']]) {
    const body = { error: { id, title: `Title of ${id}`, detail: 'Why.', field: id === 'ADM-125' ? 'grep' : null } };
    const result = await openFollow({ path: '/p', on() {}, fetcher: refusedWith(status, body) });
    assert.equal(result.reason, 'refused', id);
    assert.equal(result.error.id, id);
    assert.equal(result.error.status, status);
    assert.equal(result.error.retryAfter, null);
  }
});

test('an answer that is no stream and says nothing the console can read is said in general words', async () => {
  for (const fetcher of [refusedWith(502, 'Bad gateway'), refusedWith(500, {}), refusedWith(500, { error: {} })]) {
    const result = await openFollow({ path: '/p', on() {}, fetcher });
    assert.equal(result.reason, 'refused');
    assert.equal(result.error.title, 'The console got an answer it cannot read');
    assert.match(result.error.detail, /status 50/);
  }
  const html = await openFollow({ path: '/p', on() {}, fetcher: streamOf(['<html></html>'], { type: 'text/html' }).fetcher });
  assert.equal(html.reason, 'refused');
  assert.equal(html.error.title, 'The console got an answer it cannot read');
  assert.equal(html.error.status, 200);
});

// ---- keeping it going ----------------------------------------------------------------------------------------------

/** A follower with a fake `open`, a clock that is moved by hand and a tab that is seen or hidden. */
function setup() {
  const world = { seen: true, opened: [], states: [], events: [], gaps: 0, timers: [], now: 0 };
  const follower = makeFollower({
    open(signal, on) {
      const run = { signal, on, finish: null };
      run.done = new Promise((resolve) => { run.finish = resolve; });
      signal.addEventListener('abort', () => run.finish({ reason: 'aborted' }));
      world.opened.push(run);
      return run.done;
    },
    readGap: async () => { world.gaps += 1; },
    onEvent: (event) => world.events.push(event),
    onState: (state, detail) => world.states.push([state, detail.words ?? null]),
    visible: () => world.seen,
    later(work, wait) {
      const timer = { work, at: world.now + wait };
      world.timers.push(timer);
      return timer;
    },
    cancel(timer) {
      world.timers = world.timers.filter((one) => one !== timer);
    },
  });
  world.follower = follower;
  world.advance = (ms) => {
    world.now += ms;
    for (const timer of world.timers.filter((one) => one.at <= world.now)) {
      world.timers = world.timers.filter((one) => one !== timer);
      timer.work();
    }
  };
  /** Lets the promise callbacks run. */
  world.settle = () => new Promise((resolve) => setImmediate(resolve));
  return world;
}

test('a follow starts, hands on its events, and says so', async () => {
  const world = setup();
  world.follower.start();
  assert.equal(world.follower.state(), 'on');
  assert.deepEqual(world.states, [['on', null]]);
  assert.equal(world.opened.length, 1);
  world.opened[0].on({ kind: 'record', record: record(1) });
  assert.equal(world.events.length, 1);
  // Starting again while it goes does nothing.
  world.follower.start();
  assert.equal(world.opened.length, 1);
});

test('a follow that the stream ends says why, and does not begin again by itself', async () => {
  const world = setup();
  world.follower.start();
  world.opened[0].finish({ reason: 'ended', ended: { kind: 'ended', why: 'locked', error: '' } });
  await world.settle();
  assert.equal(world.follower.state(), 'stopped');
  assert.deepEqual(world.states.at(-1), ['stopped', 'Following stopped: you locked the console.']);
  world.advance(10 * 60_000);
  assert.equal(world.opened.length, 1);
  // The person's Follow again is a start.
  world.follower.start();
  assert.equal(world.opened.length, 2);
  assert.equal(world.follower.state(), 'on');
});

test('a refusal and a broken connection each stop it, with the error or the words', async () => {
  const world = setup();
  const error = new ApiError({ id: 'ADM-134', title: 'Too many', status: 429, retryAfter: 30 });
  let heard = null;
  const refused = makeFollower({
    open: async () => ({ reason: 'refused', error }),
    readGap() {},
    onEvent() {},
    onState: (state, detail) => { heard = [state, detail]; },
    visible: () => true,
  });
  refused.start();
  await world.settle();
  assert.equal(heard[0], 'stopped');
  assert.equal(heard[1].error, error);
  assert.equal(heard[1].words, 'Following could not start.');

  world.follower.start();
  world.opened[0].finish({ reason: 'broken' });
  await world.settle();
  assert.deepEqual(world.states.at(-1), ['stopped', 'Following stopped: the connection to the server was lost.']);

  // An open that throws is a broken connection too.
  let thrown = null;
  const throwing = makeFollower({ open: async () => { throw new Error('x'); }, readGap() {}, onEvent() {}, onState: (state, detail) => { thrown = [state, detail.words ?? null]; }, visible: () => true });
  throwing.start();
  await world.settle();
  assert.deepEqual(thrown, ['stopped', 'Following stopped: the connection to the server was lost.']);
});

test('Pause closes the stream and leaves it off, with nothing said after', async () => {
  const world = setup();
  world.follower.start();
  world.follower.stop();
  assert.equal(world.opened[0].signal.aborted, true);
  await world.settle();
  assert.equal(world.follower.state(), 'off');
  assert.deepEqual(world.states, [['on', null], ['off', null]]);
  // Pausing what is off says nothing more.
  world.follower.stop();
  assert.equal(world.states.length, 2);
});

test('a tab hidden for less than a minute keeps its follow, and one hidden for a minute closes it', async () => {
  const world = setup();
  world.follower.start();
  world.seen = false;
  world.follower.hidden();
  world.advance(HIDDEN_MS - 1);
  assert.equal(world.opened[0].signal.aborted, false);
  world.seen = true;
  world.follower.shown();
  world.advance(10 * HIDDEN_MS);
  assert.equal(world.opened[0].signal.aborted, false, 'the timer was cancelled');
  assert.equal(world.follower.state(), 'on');
  assert.equal(world.gaps, 0);

  world.seen = false;
  world.follower.hidden();
  world.advance(HIDDEN_MS);
  assert.equal(world.opened[0].signal.aborted, true);
  assert.equal(world.follower.state(), 'away');
  await world.settle();
  assert.equal(world.follower.state(), 'away', 'the stream it closed does not say it stopped');
});

test('a tab that is seen again reads the gap and follows again', async () => {
  const world = setup();
  world.follower.start();
  world.seen = false;
  world.follower.hidden();
  world.advance(HIDDEN_MS);
  world.seen = true;
  world.follower.shown();
  assert.equal(world.follower.state(), 'on');
  assert.equal(world.opened.length, 2);
  assert.equal(world.gaps, 1);
  // Seen again at once changes nothing.
  world.follower.shown();
  assert.equal(world.opened.length, 2);
  assert.equal(world.gaps, 1);
});

test('hidden is counted once, and only for a follow that goes', async () => {
  const world = setup();
  world.follower.hidden();
  assert.equal(world.timers.length, 0, 'nothing to close when it is off');
  world.follower.start();
  world.follower.hidden();
  world.follower.hidden();
  assert.equal(world.timers.length, 1);
  world.follower.stop();
  assert.equal(world.timers.length, 0, 'Pause drops the timer');
  world.follower.shown();
  assert.equal(world.opened.length, 1, 'seen again does not start what the person paused');
});

test('a follow started while the tab is hidden is closed after a minute', async () => {
  const world = setup();
  world.seen = false;
  world.follower.start();
  assert.equal(world.timers.length, 1);
  world.advance(HIDDEN_MS);
  assert.equal(world.follower.state(), 'away');
});

test('a follow that stopped by itself is not started by the tab being seen', async () => {
  const world = setup();
  world.follower.start();
  world.opened[0].finish({ reason: 'ended', ended: { kind: 'ended', why: 'time', error: '' } });
  await world.settle();
  world.follower.shown();
  assert.equal(world.opened.length, 1);
  assert.equal(world.gaps, 0);
});

test('restarting closes the stream and opens another, and the old one\'s end is not taken for a stop', async () => {
  const world = setup();
  world.follower.start();
  world.follower.restart();
  assert.equal(world.opened.length, 2);
  assert.equal(world.opened[0].signal.aborted, true);
  await world.settle();
  assert.equal(world.follower.state(), 'on');
  assert.equal(world.states.filter(([state]) => state === 'stopped').length, 0);
  // Restarting what is not following does nothing.
  world.follower.stop();
  world.follower.restart();
  assert.equal(world.opened.length, 2);
});

test('a gap that cannot be read does not stop the follow', async () => {
  const world = setup();
  const follower = makeFollower({
    open: (signal) => new Promise((resolve) => signal.addEventListener('abort', () => resolve({ reason: 'aborted' }))),
    readGap: async () => { throw new Error('no'); },
    onEvent() {},
    onState: (state) => world.states.push(state),
    visible: () => world.seen,
    later: (work, wait) => world.timers.push({ work, wait }) && world.timers.at(-1),
    cancel() {},
  });
  follower.start();
  world.seen = false;
  follower.hidden();
  world.timers[0].work();
  world.seen = true;
  follower.shown();
  await world.settle();
  assert.equal(follower.state(), 'on');
});

test('leaving the page closes the stream and says nothing', async () => {
  const world = setup();
  world.follower.start();
  world.follower.hidden();
  const before = world.states.length;
  world.follower.dispose();
  assert.equal(world.opened[0].signal.aborted, true);
  assert.equal(world.timers.length, 0);
  await world.settle();
  assert.equal(world.states.length, before);
});
