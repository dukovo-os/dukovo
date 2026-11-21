// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// lib/version-words.js: how an update's two versions are said, and whether
// they make an update of fixes only or a new version.

import assert from 'node:assert/strict';
import test from 'node:test';

import { kindOfUpdate, versionPair } from '../lib/version-words.js';

test('the two versions are said as "from to to"', () => {
  assert.equal(versionPair('8.0.3', '8.0.4'), '8.0.3 to 8.0.4');
  assert.equal(versionPair('1.0.0-alpha.1', '1.0.0-alpha.2'), '1.0.0-alpha.1 to 1.0.0-alpha.2');
});

test('a version that is not known, and two that are the same, are said once', () => {
  assert.equal(versionPair(null, '1.0.0-alpha.2'), '1.0.0-alpha.2');
  assert.equal(versionPair('8.0.3', null), '8.0.3');
  assert.equal(versionPair('', '8.0.4'), '8.0.4');
  assert.equal(versionPair('8.0.3', '8.0.3'), '8.0.3');
  assert.equal(versionPair(null, null), null);
  assert.equal(versionPair(undefined, undefined), null);
});

test('only the last of three numbers moving is an update of fixes only', () => {
  assert.equal(kindOfUpdate('8.0.3', '8.0.4'), 'fixes only');
  assert.equal(kindOfUpdate('1.30.5', '1.30.6'), 'fixes only');
  assert.equal(kindOfUpdate('8.0.9', '8.0.10'), 'fixes only', 'numbers are compared as numbers, not as words');
  assert.equal(kindOfUpdate('8.0.3', 'v8.0.4'), 'fixes only', 'a leading v is not a number');
});

test('a number before the last moving, in a version with a line, is a new version', () => {
  assert.equal(kindOfUpdate('22.1.0', '22.2.0'), 'new version');
  assert.equal(kindOfUpdate('8.0.3', '9.0.0'), 'new version');
  assert.equal(kindOfUpdate('8.0.3', '8.1.4'), 'new version', 'a new line is new even when the last number rose');
  assert.equal(kindOfUpdate('1.30.6', '1.31.0'), 'new version');
});

test('a version with fewer than three numbers has no place for a fix, so any change is a new version', () => {
  assert.equal(kindOfUpdate('2026.11', '2026.12'), 'new version');
  assert.equal(kindOfUpdate('7', '8'), 'new version');
});

test('a tag says nothing of what changes, so it is a new version', () => {
  assert.equal(kindOfUpdate('1.0.0-alpha.1', '1.0.0-alpha.2'), 'new version');
  assert.equal(kindOfUpdate('1.0.0-alpha.2', '1.0.0'), 'new version');
  assert.equal(kindOfUpdate('1.0.0', '1.0.1-rc.1'), 'new version');
});

test('a version that goes down, stays, differs in length or is not made of numbers is a new version', () => {
  assert.equal(kindOfUpdate('8.0.4', '8.0.3'), 'new version');
  assert.equal(kindOfUpdate('8.0.3', '8.0.3'), 'new version');
  assert.equal(kindOfUpdate('8.0.3', '8.0.3.1'), 'new version');
  assert.equal(kindOfUpdate('8.0.3', 'latest'), 'new version');
  assert.equal(kindOfUpdate(null, '8.0.4'), 'new version');
  assert.equal(kindOfUpdate('8.0.3', undefined), 'new version');
  assert.equal(kindOfUpdate('', ''), 'new version');
});

test('the same version with a higher build is a rebuild', () => {
  assert.equal(kindOfUpdate('15.2.0', '15.2.0_2'), 'rebuild');
  assert.equal(kindOfUpdate('15.2.0_2', '15.2.0_3'), 'rebuild');
  assert.equal(kindOfUpdate('15.2.0_9', '15.2.0_10'), 'rebuild', 'builds are compared as numbers, not as words');
  assert.equal(kindOfUpdate('2026.11_2', '2026.11_3'), 'rebuild', 'a rebuild needs no place for a fix');
});

test('a build is taken off before the versions are compared', () => {
  assert.equal(kindOfUpdate('15.2.0_2', '15.2.1'), 'fixes only');
  assert.equal(kindOfUpdate('15.2.0', '15.2.1_2'), 'fixes only');
  assert.equal(kindOfUpdate('15.2.0_2', '15.3.0'), 'new version');
  assert.equal(kindOfUpdate('15.2.0_3', '15.2.0_2'), 'new version');
  assert.equal(kindOfUpdate('15.2.0_2', '15.2.0_2'), 'new version');
  assert.equal(kindOfUpdate('15.2.0_2', '15.2.0'), 'new version');
});

test('only a build from 2 to 9999 without a leading zero is one', () => {
  assert.equal(kindOfUpdate('15.2.0', '15.2.0_1'), 'new version');
  assert.equal(kindOfUpdate('15.2.0', '15.2.0_02'), 'new version');
  assert.equal(kindOfUpdate('15.2.0', '15.2.0_10000'), 'new version');
  assert.equal(kindOfUpdate('15.2.0', '15.2.0_9999'), 'rebuild');
});
