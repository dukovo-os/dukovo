// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// lib/fuzzy.js (design 11.2): folding, each rule of the score, the places a
// word may start at, filters, ties, the cap of 50 rows and the marks.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  fold, foldedText, highlight, matchWord, MAX_ROWS, parseQuery, search,
} from '../lib/fuzzy.js';

const letters = (text) => foldedText(text).split('');
const word = (text) => [...text];
const row = (title, more = {}) => ({ title, keywords: [], facts: {}, rank: 0, recent: false, ...more });
const titles = (found) => found.map((one) => one.row.title);

test('case and accents are folded, and each folded letter knows its place in the text as written', () => {
  assert.equal(foldedText('Crème Brûlée'), 'creme brulee');
  assert.equal(foldedText('ÅNGSTRÖM ñandú'), 'angstrom nandu');
  assert.equal(foldedText('İstanbul'), 'istanbul');
  // A letter written as two code points (e and a combining accent) is one letter.
  const decomposed = fold('Cre\u0300me');
  assert.deepEqual(decomposed.letters.join(''), 'creme');
  assert.deepEqual(decomposed.at, [0, 1, 2, 4, 5]);
  // A mark with no letter before it is dropped.
  assert.equal(foldedText('\u0301a'), 'a');
});

test('a letter that is matched scores 16, and one that is not found in order gives no match', () => {
  assert.deepEqual(matchWord(word('b'), letters('ab')), { score: 16, places: [1] });
  assert.equal(matchWord(word('ba'), letters('ab')), null);
  assert.equal(matchWord(word('x'), letters('ab')), null);
});

test('a letter that starts a word of the text scores 32 more', () => {
  assert.equal(matchWord(word('b'), letters('a b')).score, 16 + 32);
  assert.equal(matchWord(word('b'), letters('a-b')).score, 16 + 32);
  assert.equal(matchWord(word('a'), letters('ab')).score, 16 + 32);
  assert.equal(matchWord(word('b'), letters('ab')).score, 16);
});

test('a letter right after the one matched before it scores 24 more', () => {
  assert.equal(matchWord(word('ab'), letters('ab')).score, (16 + 32) + (16 + 24));
  assert.equal(matchWord(word('abc'), letters('abc')).score, (16 + 32) + (16 + 24) + (16 + 24));
});

test('each letter skipped between two matches takes 3 away, and a run of skipped letters at most 30', () => {
  assert.equal(matchWord(word('ac'), letters('abc')).score, (16 + 32) + (16 - 3));
  assert.equal(matchWord(word('ad'), letters('abcd')).score, (16 + 32) + (16 - 6));
  const ten = `a${'b'.repeat(10)}z`;
  const eleven = `a${'b'.repeat(11)}z`;
  const many = `a${'b'.repeat(40)}z`;
  assert.equal(matchWord(word('az'), letters(ten)).score, (16 + 32) + (16 - 30));
  assert.equal(matchWord(word('az'), letters(eleven)).score, (16 + 32) + (16 - 30));
  assert.equal(matchWord(word('az'), letters(many)).score, (16 + 32) + (16 - 30));
});

test('the cap is for one run of skipped letters: two runs each take their own', () => {
  const text = `a${'b'.repeat(20)}c${'d'.repeat(20)}e`;
  assert.equal(matchWord(word('ace'), letters(text)).score, (16 + 32) + (16 - 30) + (16 - 30));
});

test('a word that can start at several places takes the best of the first eight', () => {
  // The second a starts a better match than the first, whose b is far away.
  const better = matchWord(word('ab'), letters('a xxxxxxxxx ab'));
  assert.deepEqual(better.places, [12, 13]);
  assert.equal(better.score, (16 + 32) + (16 + 24));
  // The eighth a is the last that is tried, and here the best.
  const eighth = matchWord(word('ab'), letters(`${'a '.repeat(7)}ab`));
  assert.deepEqual(eighth.places, [14, 15]);
  assert.equal(eighth.score, (16 + 32) + (16 + 24));
  // The ninth a is not tried: the best of the eight before it is a spread match.
  const ninth = matchWord(word('ab'), letters(`${'a '.repeat(8)}ab`));
  assert.deepEqual(ninth.places, [14, 17]);
  assert.equal(ninth.score, (16 + 32) + (16 - 6));
});

test('a query has words, and a word that is key and value filters only when its key is one rows are filtered by', () => {
  const query = parseQuery('Rest  NGI state:Failed kind:', ['state', 'kind']);
  assert.deepEqual(query.words.map((one) => one.join('')), ['rest', 'ngi']);
  assert.deepEqual(query.filters, [{ key: 'state', value: 'failed' }]);
  assert.equal(query.text, 'rest ngi');
  const unknown = parseQuery('site:photos', ['state']);
  assert.deepEqual(unknown.words.map((one) => one.join('')), ['site:photos']);
  assert.deepEqual(unknown.filters, []);
  assert.deepEqual(parseQuery('   ', ['state']), { words: [], filters: [], text: '' });
  assert.deepEqual(parseQuery('\u0301', []).words, []);
});

test('every word must be found, in any order of the words, and "rest ngi" finds Restart nginx first', () => {
  const rows = [
    row('Reload nginx settings without dropping connections', { rank: 1 }),
    row('Add a website to nginx', { rank: 1 }),
    row('Restart nginx', { rank: 1 }),
    row('nginx', { rank: 2 }),
    row('Restart a service', { rank: 1 }),
  ];
  const found = search(rows, 'rest ngi');
  assert.equal(found[0].row.title, 'Restart nginx');
  assert.ok(!titles(found).includes('Restart a service'), 'ngi is not in it');
  assert.ok(!titles(found).includes('nginx'), 'rest is not in it');
  assert.deepEqual(titles(search(rows, 'ngi rest')).slice(0, 1), ['Restart nginx']);
  assert.deepEqual(titles(search(rows, 'zzz')), []);
});

test('the row\'s score is the sum of its words, and 40 more when the title starts with the query', () => {
  const [one] = search([row('Restart nginx')], 'rest ngi');
  assert.equal(one.score, 168 + 128);
  const [alone] = search([row('Restart nginx')], 'rest');
  assert.equal(alone.score, 168 + 40);
  const [later] = search([row('The restart')], 'rest');
  assert.equal(later.score, (16 + 32) + 3 * (16 + 24));
});

test('a match in a keyword counts half, marks nothing, and loses to a better match in the title', () => {
  const [byKeyword] = search([row('Terminal', { keywords: ['shell', 'ssh'] })], 'shell');
  assert.equal(byKeyword.score, Math.floor(((16 + 32) + 4 * (16 + 24)) / 2));
  assert.deepEqual(byKeyword.marks, []);
  const [byTitle] = search([row('Shell', { keywords: ['shell'] })], 'shell');
  assert.equal(byTitle.score, ((16 + 32) + 4 * (16 + 24)) + 40);
  assert.deepEqual(byTitle.marks, [0, 1, 2, 3, 4]);
  assert.deepEqual(titles(search([row('Terminal', { keywords: ['command line'] })], 'cmd')), ['Terminal']);
});

test('the marks are the code points of the title that matched, as it was written', () => {
  assert.deepEqual(search([row('Restart nginx')], 'rest ngi')[0].marks, [0, 1, 2, 3, 8, 9, 10]);
  assert.deepEqual(search([row('Café Crème')], 'creme')[0].marks, [5, 6, 7, 8, 9]);
  // The accent that follows a marked letter is marked with it.
  assert.deepEqual(search([row('Cre\u0300me')], 'creme')[0].marks, [0, 1, 2, 3, 4, 5]);
  // A code point outside the basic plane counts once.
  assert.deepEqual(search([row('\u{1F642} Menu')], 'menu')[0].marks, [2, 3, 4, 5]);
});

test('title runs say which letters are marked', () => {
  assert.deepEqual(highlight('Restart nginx', [0, 1, 2, 3, 8, 9, 10]), [
    { text: 'Rest', hit: true }, { text: 'art ', hit: false }, { text: 'ngi', hit: true }, { text: 'nx', hit: false },
  ]);
  assert.deepEqual(highlight('\u{1F642} Menu', [2, 3, 4, 5]), [{ text: '\u{1F642} ', hit: false }, { text: 'Menu', hit: true }]);
  assert.deepEqual(highlight('Menu', []), [{ text: 'Menu', hit: false }]);
  assert.deepEqual(highlight('', []), []);
});

test('equal scores go to the recent row, then the page, the action and the thing, then the shorter title', () => {
  const same = [
    row('nginx', { rank: 2 }), row('nginx', { rank: 1 }), row('nginx', { rank: 0 }),
  ];
  assert.deepEqual(search(same, 'nginx').map((one) => one.row.rank), [0, 1, 2]);
  const recent = [row('nginx', { rank: 0 }), row('nginx', { rank: 2, recent: 0 }), row('nginx', { rank: 1, recent: 1 })];
  assert.deepEqual(search(recent, 'nginx').map((one) => one.row.rank), [2, 1, 0]);
  const lengths = [row('Restart abc'), row('Restart ab'), row('Restart abcd')];
  assert.deepEqual(titles(search(lengths, 'rest')), ['Restart ab', 'Restart abc', 'Restart abcd']);
  // Rows that tie in everything keep the order they came in.
  const alike = [row('one', { id: 1 }), row('one', { id: 2 }), row('one', { id: 3 })];
  assert.deepEqual(search(alike, 'one').map((one) => one.row.id), [1, 2, 3]);
});

test('a better score beats a recent row', () => {
  const rows = [row('Restart nginx', { recent: 0 }), row('Reload nginx')];
  assert.deepEqual(titles(search(rows, 'reload')), ['Reload nginx']);
  const both = [row('A restart', { recent: 0 }), row('Restart')];
  assert.deepEqual(titles(search(both, 'restart')), ['Restart', 'A restart']);
});

test('key:value words filter by the row\'s facts, by the start of the value, and leave the other words to match', () => {
  const rows = [
    row('nginx', { rank: 2, facts: { kind: ['thing', 'service'], state: 'running' } }),
    row('lab-broken', { rank: 2, facts: { kind: ['thing', 'service'], state: 'failed' } }),
    row('this-machine', { rank: 2, facts: { kind: ['thing', 'certificate'], state: 'expired' } }),
    row('Restart lab-broken', { rank: 1, facts: { kind: ['action', 'service'], state: 'failed' } }),
    row('Overview', { rank: 0, facts: { kind: 'page' } }),
  ];
  const options = { filterKeys: ['kind', 'state'] };
  // With nothing to match, the rows that pass come in the order they were given.
  assert.deepEqual(titles(search(rows, 'state:failed', options)), ['lab-broken', 'Restart lab-broken']);
  assert.deepEqual(titles(search(rows, 'state:fail', options)), ['lab-broken', 'Restart lab-broken']);
  assert.deepEqual(titles(search(rows, 'kind:service', options)), ['nginx', 'lab-broken', 'Restart lab-broken']);
  assert.deepEqual(titles(search(rows, 'kind:certificate', options)), ['this-machine']);
  assert.deepEqual(titles(search(rows, 'kind:page', options)), ['Overview']);
  assert.deepEqual(titles(search(rows, 'kind:service state:failed broken', options)), ['Restart lab-broken', 'lab-broken']);
  assert.deepEqual(titles(search(rows, 'State:Failed', options)), ['lab-broken', 'Restart lab-broken']);
  // A key that nothing is filtered by is a word like any other.
  assert.deepEqual(titles(search(rows, 'state:failed')), []);
  // A key with no value yet filters nothing.
  assert.equal(search(rows, 'state:', options).length, rows.length);
  // A row with no such fact does not pass.
  assert.deepEqual(titles(search(rows, 'site:photos', { filterKeys: ['site'] })), []);
});

test('no more than 50 rows are given, the best of them', () => {
  const rows = Array.from({ length: 120 }, (_, n) => row(`Row ${n}`));
  rows.push(row('Wanted'));
  assert.equal(MAX_ROWS, 50);
  assert.equal(search(rows, 'row').length, 50);
  assert.equal(search(rows, '').length, 50);
  assert.equal(search(rows, 'wanted')[0].row.title, 'Wanted');
  assert.equal(search(rows, 'row', { limit: 3 }).length, 3);
});

test('with nothing typed, every row is given: the recent ones first, newest first, and the rest as they came', () => {
  const rows = [row('b'), row('a', { recent: 1 }), row('c'), row('d', { recent: 0 })];
  assert.deepEqual(titles(search(rows, '')), ['d', 'a', 'b', 'c']);
  assert.deepEqual(titles(search(rows, '   ')), ['d', 'a', 'b', 'c']);
  assert.deepEqual(search(rows, '')[0].marks, []);
});

test('rows with no keywords or facts at all can be searched', () => {
  assert.deepEqual(titles(search([{ title: 'Plain' }], 'plain')), ['Plain']);
  assert.deepEqual(titles(search([{ title: 'Plain' }], 'kind:x', { filterKeys: ['kind'] })), []);
});
