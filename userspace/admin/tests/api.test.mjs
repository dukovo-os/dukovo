// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// api.js against a fetch that answers with what admind's section 16 of the
// console's design says it answers, and records what it was asked.

import assert from 'node:assert/strict';
import test, { afterEach } from 'node:test';

import { ApiError, get, post, setCsrf, whenSessionEnds, whenUnreachable } from '../api.js';

const realFetch = globalThis.fetch;
let calls;

function answering(status, body, headers = {}) {
  calls = [];
  globalThis.fetch = async (path, options) => {
    calls.push({ path, options });
    const text = typeof body === 'string' ? body : JSON.stringify(body);
    return new Response(status === 204 ? null : text, { status, headers });
  };
}

afterEach(() => {
  globalThis.fetch = realFetch;
  setCsrf(null);
  whenSessionEnds(null);
  whenUnreachable(null);
});

test('a GET sends the cookie and no body, token or content type', async () => {
  answering(200, { ok: true });
  setCsrf('token');
  assert.deepEqual(await get('/api/1/health'), { ok: true });
  const { path, options } = calls[0];
  assert.equal(path, '/api/1/health');
  assert.equal(options.method, 'GET');
  assert.equal(options.credentials, 'same-origin');
  assert.equal(options.body, undefined);
  assert.deepEqual(options.headers, { Accept: 'application/json' });
});

test('a POST sends its body as JSON with the session\'s token', async () => {
  answering(200, { done: true });
  setCsrf('c2Vzc2lvbg');
  assert.deepEqual(await post('/api/1/lock', { reason: 'tidy' }), { done: true });
  const { options } = calls[0];
  assert.equal(options.method, 'POST');
  assert.equal(options.credentials, 'same-origin');
  assert.equal(options.body, '{"reason":"tidy"}');
  assert.equal(options.headers['Content-Type'], 'application/json');
  assert.equal(options.headers['X-Dukovo-CSRF'], 'c2Vzc2lvbg');
});

test('a POST before there is a session has no token and still has a body', async () => {
  answering(200, { done: true });
  await post('/api/1/sign-in/passkey/options');
  const { options } = calls[0];
  assert.equal(options.body, '{}');
  assert.equal('X-Dukovo-CSRF' in options.headers, false);
});

test('an answer with no body is null', async () => {
  answering(204, '');
  assert.equal(await post('/api/1/sign-out'), null);
});

test('an error is thrown with the id, words and status admind gave', async () => {
  answering(401, {
    error: {
      id: 'ADM-101',
      title: 'That name or password is not right',
      detail: 'Check them and try again.',
      fix: 'The account name is the one used at the console.',
    },
  });
  await assert.rejects(post('/api/1/sign-in', { name: 'chase', password: 'x' }), (error) => {
    assert.ok(error instanceof ApiError);
    assert.deepEqual(
      { id: error.id, title: error.title, detail: error.detail, fix: error.fix, status: error.status, retryAfter: error.retryAfter },
      {
        id: 'ADM-101',
        title: 'That name or password is not right',
        detail: 'Check them and try again.',
        fix: 'The account name is the one used at the console.',
        status: 401,
        retryAfter: null,
      },
    );
    assert.equal(error.message, error.title);
    return true;
  });
});

test('an error without a fix has an empty one', async () => {
  answering(400, { error: { id: 'ADM-120', title: 'No name', detail: 'Give one.' } });
  await assert.rejects(post('/api/1/sign-in', {}), { fix: '', status: 400 });
});

test('a 429 says how many seconds to wait', async () => {
  answering(429, { error: { id: 'ADM-105', title: 'Wait a little', detail: 'Too many tries.' } }, { 'Retry-After': '45' });
  await assert.rejects(post('/api/1/sign-in', {}), { id: 'ADM-105', status: 429, retryAfter: 45 });
});

test('a Retry-After that is a date is not taken for seconds', async () => {
  answering(429, { error: { id: 'ADM-105', title: 'Wait', detail: '' } }, { 'Retry-After': 'Fri, 13 Nov 2026 23:59:59 GMT' });
  await assert.rejects(post('/api/1/sign-in', {}), { retryAfter: null });
});

test('an error answer that is not admind\'s shape still says what happened', async () => {
  answering(502, '<html>Bad gateway</html>');
  await assert.rejects(get('/api/1/session'), (error) => {
    assert.equal(error.id, null);
    assert.equal(error.status, 502);
    assert.match(error.title, /answer it cannot read/);
    assert.match(error.detail, /status 502/);
    return true;
  });
});

test('no answer at all is an error with status 0', async () => {
  calls = [];
  globalThis.fetch = async () => {
    throw new TypeError('Failed to fetch');
  };
  await assert.rejects(get('/api/1/session'), { name: 'ApiError', status: 0, id: null, title: /cannot reach the server/ });
});

test('the session ending is reported once for each answer that says so', async () => {
  const heard = [];
  whenSessionEnds((error) => heard.push(error.id));
  answering(401, { error: { id: 'ADM-107', title: 'You were signed out', detail: 'The console restarted.' } });
  await assert.rejects(get('/api/1/session'), { id: 'ADM-107' });
  assert.deepEqual(heard, ['ADM-107']);
  answering(401, { error: { id: 'ADM-101', title: 'Wrong', detail: '' } });
  await assert.rejects(post('/api/1/sign-in', {}), { id: 'ADM-101' });
  assert.deepEqual(heard, ['ADM-107']);
});

test('an error keeps the field a value was refused for and the program\'s own words', async () => {
  const command = { argv: ['service', 'restart', 'nginx'], status: 1, stderr: 'nginx: unknown directive' };
  answering(422, { error: { id: 'ADM-123', title: 'nginx could not restart', detail: 'See below.', command } });
  await assert.rejects(post('/api/1/services/nginx/restart', {}), { field: null, command });
  answering(400, { error: { id: 'ADM-125', title: 'That port cannot work', detail: 'Ports go from 1 to 65535.', field: 'port' } });
  await assert.rejects(post('/api/1/firewall', {}), { field: 'port', command: null });
});

test('a signal ends the wait and not the call, whose answer is still read', async () => {
  let answer;
  let read = false;
  calls = [];
  globalThis.fetch = (path, options) => {
    calls.push({ path, options });
    return new Promise((resolve) => {
      answer = () => resolve({ ok: true, status: 200, json: async () => { read = true; return {}; } });
    });
  };
  const own = new AbortController();
  const waiting = get('/api/1/services', { signal: own.signal });
  own.abort();
  await assert.rejects(waiting, { name: 'AbortError' });
  // The browser was given nothing to abort the request with.
  assert.equal('signal' in calls[0].options, false);
  answer();
  await new Promise((resolve) => { setTimeout(resolve, 0); });
  assert.equal(read, true);
  // A signal that has already ended sends nothing.
  await assert.rejects(post('/api/1/lock', {}, { signal: own.signal }), { name: 'AbortError' });
  assert.equal(calls.length, 1);
});

test('a call whose page has gone is not taken for a server that is gone, nor for one that answered', async () => {
  const heard = [];
  whenUnreachable((error) => heard.push(error.status));
  const own = new AbortController();
  globalThis.fetch = async () => {
    own.abort();
    return { ok: true, status: 200, json: async () => ({}) };
  };
  await assert.rejects(get('/api/1/tasks', { signal: own.signal }), { name: 'AbortError' });
  assert.deepEqual(heard, []);
  // An answer that comes in time is the page's, with or without a signal.
  answering(200, { n: 1 });
  assert.deepEqual(await get('/api/1/tasks', { signal: new AbortController().signal }), { n: 1 });
});

test('each request that gets no answer is reported to whoever watches for a silent server', async () => {
  const heard = [];
  whenUnreachable((error) => heard.push(error.status));
  globalThis.fetch = async () => {
    throw new TypeError('Failed to fetch');
  };
  await assert.rejects(get('/api/1/session'), { status: 0 });
  await assert.rejects(post('/api/1/lock', {}), { status: 0 });
  assert.deepEqual(heard, [0, 0]);
});
