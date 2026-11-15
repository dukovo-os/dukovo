// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The log's card (design 4.6): the sentences about its use and limits, the
// form's start, what a size is, and what is sent when a limit changes.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  formOf, keepWords, logArgs, logWords, parseSize, sizeText,
} from '../pages/storage/log-model.js';

const LOG = {
  usage: { bytes: 1_288_490_188, files: 92, audit_bytes: 83_886_080, directory: '/system/state/log' },
  limits: { keep_days: 90, max_bytes: 4_294_967_296, max_percent: 10, audit_keep_days: 365 },
  cap: 4_294_967_296,
  filesystem_size: 255_999_999_999,
  wrong: [],
};

test('the log\'s use and limits are two sentences', () => {
  assert.deepEqual(logWords(LOG), {
    use: 'The system\'s log takes 1.29 GB in 92 files, 83.9 MB of it the audit log.'.replace('83.9 MB', '84 MB'),
    keeps: 'It keeps 90 days of records, and at most 4.29 GB or 10 percent of its disk, whichever is less; the audit log keeps a year.',
    wrong: [],
  });
});

test('a log with a limit lifted says so', () => {
  const words = (limits) => logWords({ ...LOG, limits: { ...LOG.limits, ...limits } }).keeps;
  assert.equal(words({ keep_days: 0 }), 'It keeps records for as long as there is room, and at most 4.29 GB or 10 percent of its disk, whichever is less; the audit log keeps a year.');
  assert.equal(words({ max_percent: 0 }), 'It keeps 90 days of records, and at most 4.29 GB; the audit log keeps a year.');
  assert.equal(words({ max_bytes: 0 }), 'It keeps 90 days of records, and at most 10 percent of its disk; the audit log keeps a year.');
  assert.equal(words({ max_bytes: 0, max_percent: 0, audit_keep_days: 0 }), 'It keeps 90 days of records; the audit log keeps its records for good.');
  assert.equal(words({ audit_keep_days: 400 }), 'It keeps 90 days of records, and at most 4.29 GB or 10 percent of its disk, whichever is less; the audit log keeps 400 days.');
  assert.equal(keepWords(1), '1 day');
});

test('the settings file\'s mistakes are given as they are', () => {
  assert.deepEqual(logWords({ ...LOG, wrong: ['line 3: keep-days is not a number'] }).wrong, ['line 3: keep-days is not a number']);
  assert.deepEqual(logWords(null).wrong, []);
});

test('the form starts with the limits, and no limit is an empty field', () => {
  assert.deepEqual(formOf(LOG.limits), { keep_days: '90', max_size: '4G', max_percent: '10', audit_keep_days: '365' });
  assert.deepEqual(formOf({ keep_days: 0, max_bytes: 0, max_percent: 0, audit_keep_days: 0 }), { keep_days: '', max_size: '', max_percent: '', audit_keep_days: '' });
  assert.equal(sizeText(1536), '1536');
  assert.equal(sizeText(2 * 1024 ** 2), '2M');
  assert.equal(sizeText(1024 ** 4), '1T');
});

test('a size is read as logtrim reads it', () => {
  assert.equal(parseSize('4G'), 4 * 1024 ** 3);
  assert.equal(parseSize('4g'), 4 * 1024 ** 3);
  assert.equal(parseSize('4GiB'), 4 * 1024 ** 3);
  assert.equal(parseSize('4GB'), 4e9);
  assert.equal(parseSize('500M'), 500 * 1024 ** 2);
  assert.equal(parseSize('1000'), 1000);
  assert.equal(parseSize(' 2T '), 2 * 1024 ** 4);
  assert.equal(parseSize('5B'), null);
  assert.equal(parseSize('lots'), null);
  assert.equal(parseSize('4 G'), null);
  assert.equal(parseSize('-4G'), null);
  assert.equal(parseSize(''), null);
  assert.equal(parseSize('99999999999P'), null);
});

test('only the fields that changed are sent, and a lifted limit is 0', () => {
  const same = formOf(LOG.limits);
  assert.deepEqual(logArgs(same, LOG.limits), { nothing: true });
  assert.deepEqual(logArgs({ ...same, keep_days: '60' }, LOG.limits), { args: { keep_days: 60 } });
  assert.deepEqual(logArgs({ ...same, max_size: '8G', max_percent: '20' }, LOG.limits), { args: { max_size: '8G', max_percent: 20 } });
  assert.deepEqual(logArgs({ ...same, max_size: '4096M' }, LOG.limits), { nothing: true }, 'the same size in other words');
  assert.deepEqual(logArgs({ ...same, keep_days: '', max_size: 'no limit', max_percent: '0', audit_keep_days: ' ' }, LOG.limits), {
    args: { keep_days: 0, max_size: '0', max_percent: 0, audit_keep_days: 0 },
  });
});

test('a limit that is already lifted and left empty is nothing to change', () => {
  const lifted = { keep_days: 0, max_bytes: 0, max_percent: 0, audit_keep_days: 0 };
  assert.deepEqual(logArgs(formOf(lifted), lifted), { nothing: true });
  assert.deepEqual(logArgs({ ...formOf(lifted), keep_days: '30' }, lifted), { args: { keep_days: 30 } });
});

test('a field out of its range or not a number is an error under that field, and nothing is sent', () => {
  const same = formOf(LOG.limits);
  const errors = (change) => logArgs({ ...same, ...change }, LOG.limits).errors;
  assert.deepEqual(Object.keys(errors({ keep_days: '3651' })), ['keep_days']);
  assert.deepEqual(Object.keys(errors({ keep_days: '1.5' })), ['keep_days']);
  assert.deepEqual(Object.keys(errors({ max_percent: '101' })), ['max_percent']);
  assert.deepEqual(Object.keys(errors({ audit_keep_days: '29' })), ['audit_keep_days']);
  assert.deepEqual(Object.keys(errors({ max_size: 'lots', keep_days: '-1' })), ['keep_days', 'max_size']);
  assert.equal(errors({ keep_days: '3650' }), undefined);
  assert.equal(errors({ audit_keep_days: '30' }), undefined);
  assert.equal(errors({ max_percent: '100' }), undefined);
});
