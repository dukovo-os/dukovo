// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// lib/problem-model.js: each row of the design's table of where an error
// goes, the words of the error view, and that every error ID the module
// names has a page in docs/errors.

import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import test from 'node:test';

import { ApiError } from '../api.js';
import {
  detailsText, errorParts, helpPath, inPlaceWords, missingProgram, NAMED_IDS, PLACE, placeOf, waitWords,
} from '../lib/problem-model.js';

const error = (more) => new ApiError({ title: 'Something', detail: 'It happened.', status: 400, ...more });

test('a value that cannot work goes under its field, and without a field it is an error like any other', () => {
  assert.equal(placeOf(error({ id: 'ADM-125', field: 'port', status: 400 })), PLACE.field);
  assert.equal(placeOf(error({ id: 'ADM-125', field: null })), PLACE.view);
});

test('an account that may look but not change gets a banner in the page\'s head', () => {
  assert.equal(placeOf(error({ id: 'ADM-122', status: 403 })), PLACE.banner);
});

test('a locked console asks for the unlock, and a console that signed the person out shows the sign-in page', () => {
  assert.equal(placeOf(error({ id: 'ADM-106', status: 403 })), PLACE.unlock);
  assert.equal(placeOf(error({ id: 'ADM-107', status: 401 })), PLACE.signIn);
});

test('too many tries waits, by its ID or by its status', () => {
  assert.equal(placeOf(error({ id: 'ADM-105', status: 429, retryAfter: 40 })), PLACE.wait);
  assert.equal(placeOf(error({ id: null, status: 429 })), PLACE.wait);
});

test('a helper that stopped, or a bad gateway, says so and offers another try', () => {
  assert.equal(placeOf(error({ id: 'ADM-111', status: 502 })), PLACE.helper);
  assert.equal(placeOf(error({ id: 'ADM-112', status: 502 })), PLACE.helper);
  assert.equal(placeOf(error({ id: null, status: 502 })), PLACE.helper);
});

test('no answer at all is the shell\'s offline state, not an error to show', () => {
  assert.equal(placeOf(new ApiError({ title: 'The console cannot reach the server' })), PLACE.offline);
  // An answer with no admind error in it is not "no answer".
  assert.equal(placeOf(error({ id: null, status: 500 })), PLACE.view);
});

test('a command this server does not have yet is said in place, plainly', () => {
  assert.equal(placeOf(error({ id: 'ADM-129', status: 501 })), PLACE.inPlace);
});

test('ADM-123 and every other error go to the error view', () => {
  assert.equal(placeOf(error({ id: 'ADM-123', status: 422 })), PLACE.view);
  assert.equal(placeOf(error({ id: 'SVC-104', status: 422 })), PLACE.view);
  assert.equal(placeOf(error({ id: 'ADM-121', status: 500 })), PLACE.view);
  assert.equal(placeOf(error({ id: 'ADM-127', status: 404 })), PLACE.view);
});

test('the wait says how long in the console\'s forms of time', () => {
  assert.equal(waitWords(40), 'Too many tries. Try again in 40 s.');
  assert.equal(waitWords(90), 'Too many tries. Try again in 2 min.');
});

test('the program a missing command names is the one in backticks, and the words say so', () => {
  const named = error({ id: 'ADM-129', detail: 'The program `dukovo-check` is not on this server.' });
  assert.equal(missingProgram(named), 'dukovo-check');
  assert.equal(inPlaceWords(named), 'This needs dukovo-check, which this server does not have yet.');
  const unnamed = error({ id: 'ADM-129', detail: 'There is no program for it.' });
  assert.equal(missingProgram(unnamed), null);
  assert.equal(inPlaceWords(unnamed), 'This needs a part of Dukovo that this server does not have yet.');
});

test('the error view has what happened, what to do and the program\'s own words, each only when there is one', () => {
  const full = errorParts(error({
    id: 'ADM-123',
    title: 'nginx could not restart',
    detail: 'nginx: unknown directive',
    fix: 'Fix the setting and try again.',
    command: { argv: ['service', 'restart', 'nginx'], status: 1, stderr: 'nginx: [emerg] unknown directive\n' },
  }));
  assert.deepEqual(full, {
    title: 'nginx could not restart',
    happened: 'nginx: unknown directive',
    todo: 'Fix the setting and try again.',
    ownWords: 'nginx: [emerg] unknown directive',
    id: 'ADM-123',
    helpPath: '/help/errors/ADM-123',
    helpWords: 'What ADM-123 means',
  });
  const bare = errorParts(new ApiError({ title: 'The console got an answer it cannot read' }));
  assert.deepEqual(bare, {
    title: 'The console got an answer it cannot read',
    happened: null,
    todo: null,
    ownWords: null,
    id: null,
    helpPath: null,
    helpWords: null,
  });
  assert.equal(errorParts(error({ command: { argv: [], status: 2, stderr: '  ' } })).ownWords, null);
  assert.equal(helpPath('ADM-106'), '/help/errors/ADM-106');
});

test('the details to copy hold the ID, the words, the command and its status, the page and the time', () => {
  const text = detailsText(
    error({
      id: 'ADM-123',
      title: 'nginx could not restart',
      detail: 'nginx: unknown directive',
      fix: 'Fix the setting.',
      status: 422,
      command: { argv: ['service', 'restart', 'nginx'], status: 1, stderr: 'nginx: [emerg] unknown directive\n' },
    }),
    { path: '/services/nginx', at: 'Nov 14, 2026, 3:12:00 AM' },
  );
  assert.equal(text, [
    'ADM-123: nginx could not restart',
    'nginx: unknown directive',
    'Fix the setting.',
    'Command: service restart nginx',
    'Status: 1',
    'Its message:',
    'nginx: [emerg] unknown directive',
    'HTTP status: 422',
    'Page: /services/nginx',
    'Time: Nov 14, 2026, 3:12:00 AM',
  ].join('\n'));
  const plain = detailsText(new ApiError({ title: 'The console cannot reach the server' }), { path: '/', at: 'now' });
  assert.equal(plain, 'The console cannot reach the server\nPage: /\nTime: now');
});

test('every error ID the module names has a page in docs/errors', () => {
  const folder = new URL('../../../docs/errors/', import.meta.url);
  assert.ok(NAMED_IDS.length >= 9);
  for (const id of NAMED_IDS) {
    assert.ok(existsSync(new URL(`${id}.md`, folder)), `${id} has no page in docs/errors`);
    assert.match(readFileSync(new URL(`${id}.md`, folder), 'utf8'), new RegExp(`^# ${id}: `, 'm'));
  }
});
