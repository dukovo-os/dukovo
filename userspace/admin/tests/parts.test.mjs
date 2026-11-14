// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// shell/parts.js: parts are called with a view of reads only, a part that
// throws or is late is left out and named, a missing one is skipped, and
// the cache gives one answer to every part that asks until it is forgotten.

import assert from 'node:assert/strict';
import test from 'node:test';

import { makeParts } from '../shell/parts.js';

const logs = () => {
  const warned = [];
  const noted = [];
  return { warned, noted, warn: (...words) => warned.push(words.join(' ')), note: (...words) => noted.push(words.join(' ')) };
};

const entry = (id, parts) => ({ id, parts });

function setup(pages, more = {}) {
  const log = logs();
  const calls = [];
  const parts = makeParts({
    pages,
    get: async (path, options) => {
      calls.push([path, Boolean(options.signal)]);
      return { path };
    },
    session: () => ({ name: 'alice' }),
    time: () => 'time',
    warn: log.warn,
    note: log.note,
    ...more,
  });
  return { parts, calls, log };
}

test('each page with the part is asked, and the answer is kept with its entry', async () => {
  const one = entry('one', { overview: async () => ({ needs: async (cx) => [{ id: 'a', who: cx.session.name, time: cx.time }] }) });
  const two = entry('two', {});
  const { parts } = setup([one, two]);
  const found = await parts.collect('overview', (part, cx) => part.needs(cx));
  assert.deepEqual(found, [{ entry: one, value: [{ id: 'a', who: 'alice', time: 'time' }] }]);
});

test('a part reads with a signal, and the same read asked by three parts is one call', async () => {
  const reads = async () => ({ needs: (cx) => cx.cache('services', 1000, () => cx.api.get('/api/1/services')) });
  const { parts, calls } = setup([entry('a', { overview: reads }), entry('b', { overview: reads }), entry('c', { overview: reads })]);
  const found = await parts.collect('overview', (part, cx) => part.needs(cx));
  assert.equal(found.length, 3);
  assert.deepEqual(calls, [['/api/1/services', true]]);
});

test('the cache keeps an answer for its time only, and not a failure', async () => {
  let clock = 0;
  let count = 0;
  let fail = true;
  const part = async () => ({
    needs: (cx) => cx.cache('k', 100, async () => {
      count += 1;
      if (fail) throw new Error('no');
      return count;
    }),
  });
  const { parts } = setup([entry('a', { overview: part })], { now: () => clock, warn() {} });
  assert.deepEqual(await parts.collect('overview', (one, cx) => one.needs(cx)), []);
  fail = false;
  assert.equal((await parts.collect('overview', (one, cx) => one.needs(cx)))[0].value, 2, 'a failure is asked again');
  clock = 50;
  assert.equal((await parts.collect('overview', (one, cx) => one.needs(cx)))[0].value, 2, 'kept');
  clock = 150;
  assert.equal((await parts.collect('overview', (one, cx) => one.needs(cx)))[0].value, 3, 'asked again after its time');
});

test('forget drops what the cache kept, so the next read asks again', async () => {
  let count = 0;
  const part = async () => ({
    needs: (cx) => cx.cache('k', 60_000, async () => {
      count += 1;
      return count;
    }),
  });
  const { parts } = setup([entry('a', { overview: part })], { now: () => 0 });
  assert.equal((await parts.collect('overview', (one, cx) => one.needs(cx)))[0].value, 1);
  assert.equal((await parts.collect('overview', (one, cx) => one.needs(cx)))[0].value, 1, 'kept');
  parts.forget();
  assert.equal((await parts.collect('overview', (one, cx) => one.needs(cx)))[0].value, 2, 'asked again after forget');
});

test('a part that throws is left out and named with its page, and the others still answer', async () => {
  const { parts, log } = setup([
    entry('bad', { overview: async () => ({ needs: () => { throw new Error('broken'); } }) }),
    entry('good', { overview: async () => ({ needs: () => ['fine'] }) }),
  ]);
  const found = await parts.collect('overview', (part, cx) => part.needs(cx));
  assert.deepEqual(found.map((one) => one.entry.id), ['good']);
  assert.equal(log.warned.length, 1);
  assert.match(log.warned[0], /overview part of the page bad was left out/);
});

test('a part that runs out of time is left out', async () => {
  const slow = async () => ({ needs: () => new Promise(() => {}) });
  const { parts, log } = setup([entry('slow', { badge: slow })], { limit: 20 });
  assert.deepEqual(await parts.collect('badge', (part, cx) => part.count(cx)), []);
  assert.match(log.warned[0], /badge part of the page slow/);
});

test('a part that is not there is skipped without a warning, and a part without the function is left out', async () => {
  const { parts, log } = setup([
    entry('missing', { overview: async () => { throw new Error('Failed to fetch'); } }),
    entry('wrong', { overview: async () => ({}) }),
  ]);
  assert.deepEqual(await parts.collect('overview', (part, cx) => part.needs(cx)), []);
  assert.equal(log.noted.length, 1);
  assert.match(log.noted[0], /missing has no overview part yet/);
  assert.equal(log.warned.length, 1);
  assert.match(log.warned[0], /wrong/);
});
