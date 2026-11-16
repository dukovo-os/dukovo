// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// lib/health.js: the weight of every check of design 10.1's table, the
// floor at 0, the cap of SEC-150, a check the table does not know, and the
// three bands.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  assess, bandOf, bandRanges, BANDS, OTHER_WEIGHT, SERVICE_CAP, weightOf, weightRows, WEIGHTS,
} from '../lib/health.js';

const found = (id, result = 'warning', more = {}) => ({ area: 'x', check: 'y', result, detail: `${id} is so`, id, fix: 'Do this.', ...more });
const kept = (...results) => ({ version: 1, checked: '2026-11-20T05:17:03Z', release: '1.0.0-alpha.1', results });

test('each check of the table weighs what the design says', () => {
  const table = {
    'SEC-101': 10, 'SEC-102': 15, 'SEC-110': 6, 'SEC-111': 4, 'SEC-112': 6, 'SEC-120': 20,
    'SEC-121': 10, 'SEC-130': 6, 'SEC-131': 4, 'SEC-140': 4, 'SEC-150': 4, 'SEC-160': 8, 'SEC-170': 6,
  };
  assert.deepEqual(Object.fromEntries(Object.keys(WEIGHTS).map((id) => [id, weightOf(id)])), table);
  for (const [id, weight] of Object.entries(table)) {
    assert.equal(assess(kept(found(id))).score, 100 - weight, `${id} as a warning`);
  }
});

test('an early warning weighs 4 and a hardware check 6, each', () => {
  assert.equal(weightOf('MET-104'), 4);
  assert.equal(weightOf('MET-201'), 4);
  assert.equal(weightOf('HW-113'), 6);
  assert.equal(assess(kept(found('MET-104'), found('MET-105'))).score, 92);
  assert.equal(assess(kept(found('HW-113'), found('HW-120'))).score, 88);
});

test('a check the table does not know weighs 3, and one with no ID does too', () => {
  assert.equal(OTHER_WEIGHT, 3);
  assert.equal(weightOf('SEC-999'), 3);
  assert.equal(weightOf('NEW-1'), 3);
  assert.equal(weightOf(null), 3);
  assert.equal(weightOf(undefined), 3);
  assert.equal(assess(kept(found('SEC-999'))).score, 97);
  assert.equal(assess(kept(found(undefined))).score, 97);
  assert.equal(assess(kept(found(''))).findings[0].id, null);
});

test('a FAILED takes off twice the weight, and any case of the word counts', () => {
  assert.equal(assess(kept(found('SEC-110', 'FAILED'))).score, 88);
  assert.equal(assess(kept(found('SEC-110', 'failed'))).score, 88);
  assert.equal(assess(kept(found('SEC-110', 'Warning'))).score, 94);
  assert.equal(assess(kept(found('SEC-999', 'FAILED'))).score, 94);
});

test('a passed or skipped check takes nothing off, and a word it does not know is not a pass', () => {
  assert.equal(assess(kept(found('SEC-110', 'passed'), found('SEC-120', 'skipped'))).score, 100);
  const odd = assess(kept(found('SEC-110', 'error')));
  assert.equal(odd.score, 94);
  assert.equal(odd.findings[0].severity, 'warn');
  assert.equal(assess(kept(found('SEC-110', 'warning', { result: undefined }))).score, 100, 'no word is no finding');
});

test('the score never goes under 0', () => {
  const worst = kept(...Object.keys(WEIGHTS).map((id) => found(id, 'FAILED')), found('HW-1', 'FAILED'));
  assert.equal(assess(worst).score, 0);
  assert.equal(assess(kept(found('SEC-120', 'FAILED'), found('SEC-102', 'FAILED'), found('SEC-101', 'FAILED'))).score, 10);
  assert.equal(assess(kept(found('SEC-120', 'FAILED'), found('SEC-102', 'FAILED'), found('SEC-101', 'FAILED'), found('SEC-121', 'FAILED'))).score, 0);
});

test('SEC-150 takes off at most 12 however many services gave up', () => {
  assert.equal(SERVICE_CAP, 12);
  const gave = (count, result = 'warning') => assess(kept(...Array.from({ length: count }, () => found('SEC-150', result))));
  assert.equal(gave(1).score, 96);
  assert.equal(gave(3).score, 88);
  assert.equal(gave(4).score, 88);
  assert.equal(gave(9).score, 88);
  assert.equal(gave(2, 'FAILED').score, 88, 'two that failed are 16, held to 12');
  assert.equal(gave(9).findings.length, 9, 'each one is still a finding');
  // The cap is its own: other findings are counted in full.
  assert.equal(assess(kept(...Array.from({ length: 5 }, () => found('SEC-150')), found('SEC-110'))).score, 82);
});

test('the bands are 90 and over, 70 to 89, and under 70, with words as well as colours', () => {
  assert.deepEqual(BANDS.map((band) => [band.id, band.least, band.words]), [
    ['ok', 90, 'Good'], ['warn', 70, 'Could be better'], ['bad', 0, 'Needs you'],
  ]);
  for (const [score, id] of [[100, 'ok'], [90, 'ok'], [89, 'warn'], [70, 'warn'], [69, 'bad'], [0, 'bad']]) {
    assert.equal(bandOf(score).id, id, String(score));
  }
  assert.equal(assess(kept()).band.words, 'Good');
  assert.equal(assess(kept(found('SEC-110'), found('SEC-130'))).band.words, 'Could be better');
  assert.equal(assess(kept(found('SEC-120'), found('SEC-102'), found('SEC-101'))).band.words, 'Needs you');
});

test('each band says what it covers in words', () => {
  assert.deepEqual(bandRanges().map((band) => [band.id, band.words, band.range]), [
    ['ok', 'Good', '90 and over'], ['warn', 'Could be better', '70 to 89'], ['bad', 'Needs you', 'under 70'],
  ]);
});

test('the findings come worst first, in an order that does not change between two reads', () => {
  const result = assess(kept(
    found('SEC-131'), found('SEC-130'), found('SEC-140', 'FAILED'), found('SEC-102'), found('SEC-110'), found('SEC-111'),
  ));
  assert.deepEqual(result.findings.map((one) => [one.id, one.severity, one.points]), [
    ['SEC-140', 'bad', 8], ['SEC-102', 'warn', 15], ['SEC-110', 'warn', 6], ['SEC-130', 'warn', 6], ['SEC-111', 'warn', 4], ['SEC-131', 'warn', 4],
  ]);
});

test('a finding keeps the words the check gave, and one with none has empty words', () => {
  const [one] = assess(kept(found('SEC-110', 'warning', { area: 'ssh', check: 'passwords' }))).findings;
  assert.deepEqual(one, {
    id: 'SEC-110', area: 'ssh', check: 'passwords', detail: 'SEC-110 is so', fix: 'Do this.', severity: 'warn', points: 6,
  });
  const [bare] = assess(kept({ result: 'warning' })).findings;
  assert.deepEqual([bare.area, bare.check, bare.detail, bare.fix], ['', '', '', '']);
});

test('the time of the checks is read, and none is none', () => {
  assert.equal(assess(kept()).checked, Date.UTC(2026, 10, 20, 5, 17, 3) / 1000);
  assert.equal(assess({ results: [] }).checked, null);
  assert.equal(assess({ checked: 'not a time', results: [] }).checked, null);
});

test('a result that is not there or has no list of results is a machine with nothing found', () => {
  for (const result of [null, undefined, {}, { results: null }, { results: 'x' }]) {
    const answer = assess(result);
    assert.equal(answer.score, 100);
    assert.deepEqual(answer.findings, []);
  }
});

test('the table in the dialog has a row for every weight, the prefixes and the rest', () => {
  const rows = weightRows();
  assert.equal(rows.length, Object.keys(WEIGHTS).length + 3);
  assert.deepEqual(rows.slice(-3).map((row) => [row.id, row.weight]), [['MET-…', 4], ['HW-…', 6], ['Any other', 3]]);
  assert.ok(rows.every((row) => row.words && Number.isInteger(row.weight)));
});
