// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Matching for the palette (design 11.2), as plain values. Text is folded
// before it is compared: accents and case do not count. Each word typed must
// be found, letter by letter in order, in the row's title or in one of its
// keywords; the words need not come in the order of the title. A word that
// is `key:value`, with a key that rows can be filtered by, filters instead.
//
// A word is scored on the letters that matched:
//
//   +16  each letter matched
//   +32  a letter that starts a word of the text
//   +24  a letter right after the one matched before it
//   -3   each letter skipped between two matches of the word, at most -30
//        for one run of skipped letters
//
// A word can start at any place its first letter is found; the first
// eight places are tried and the best score counts. A match in a keyword
// counts half. The row's score is the sum over its words, and 40 more
// when its title starts with what was typed. Ties go to the recent row,
// then the page, the action and the thing, then the shorter title.

/** The most rows a search gives. */
export const MAX_ROWS = 50;

// How many places a word may start at.
const START_PLACES = 8;

const PER_LETTER = 16;
const STARTS_WORD = 32;
const FOLLOWS = 24;
const PER_SKIPPED = 3;
const MOST_SKIPPED = 30;
const STARTS_TEXT = 40;

const MARKS = /\p{M}/gu;
const WORD_LETTER = /[\p{L}\p{N}]/u;

/** One code point folded: its canonical decomposition with the marks taken away, in lower case. May be empty, or more than one letter. */
const foldPoint = (point) => point.normalize('NFD').replace(MARKS, '').toLowerCase();

/**
 * `text` folded. `letters` are the folded code points; `at[i]` is the
 * index, among the code points of `text`, of the one that `letters[i]`
 * came from, so that a match can be marked in the text as it was written;
 * `length` is the number of code points of `text`.
 */
export function fold(text) {
  const letters = [];
  const at = [];
  let index = 0;
  for (const point of text) {
    for (const letter of foldPoint(point)) {
      letters.push(letter);
      at.push(index);
    }
    index += 1;
  }
  return { letters, at, length: index };
}

/** The code points of the text as written that the letter at `place` stands for: its own, and the accents after it that folding took away. */
function codePointsOf(folded, place) {
  const from = folded.at[place];
  const to = place + 1 < folded.at.length ? folded.at[place + 1] : folded.length;
  return Array.from({ length: to - from }, (_, n) => from + n);
}

/** `text` folded, as a string. */
export const foldedText = (text) => fold(text).letters.join('');

/** The score and the places (indexes into `letters`) of a match of `word` that starts at `first`, or null. */
function follow(word, letters, first) {
  const places = [first];
  let score = PER_LETTER;
  if (first === 0 || !WORD_LETTER.test(letters[first - 1])) score += STARTS_WORD;
  let last = first;
  for (let n = 1; n < word.length; n += 1) {
    const next = letters.indexOf(word[n], last + 1);
    if (next === -1) return null;
    score += PER_LETTER;
    if (next === last + 1) {
      score += FOLLOWS;
    } else {
      score -= Math.min((next - last - 1) * PER_SKIPPED, MOST_SKIPPED);
    }
    if (!WORD_LETTER.test(letters[next - 1])) score += STARTS_WORD;
    places.push(next);
    last = next;
  }
  return { score, places };
}

/**
 * Matches `word`, an array of folded letters, in the folded `letters` of
 * some text: the best of the first eight places it can start at, as
 * `{score, places}`, or null when its letters are not all there in order.
 */
export function matchWord(word, letters) {
  let best = null;
  let tried = 0;
  for (let from = letters.indexOf(word[0]); from !== -1 && tried < START_PLACES; from = letters.indexOf(word[0], from + 1)) {
    tried += 1;
    const found = follow(word, letters, from);
    // A later start has fewer letters left to find the rest in.
    if (found === null) break;
    if (best === null || found.score > best.score) best = found;
  }
  return best;
}

/**
 * What was typed: `words`, each an array of folded letters; `filters`, the
 * `{key, value}` of each word of the form `key:value` whose key is one of
 * `filterKeys`, folded; and `text`, the words as one folded string. A key
 * with no value yet filters nothing and is not a word.
 */
export function parseQuery(query, filterKeys = []) {
  const words = [];
  const filters = [];
  for (const typed of query.trim().split(/\s+/)) {
    const colon = typed.indexOf(':');
    const key = colon > 0 ? foldedText(typed.slice(0, colon)) : '';
    if (key && filterKeys.includes(key)) {
      const value = foldedText(typed.slice(colon + 1));
      if (value) filters.push({ key, value });
    } else {
      const letters = fold(typed).letters;
      if (letters.length > 0) words.push(letters);
    }
  }
  return { words, filters, text: words.map((letters) => letters.join('')).join(' ') };
}

// What a row keeps folded, so that a keystroke folds nothing again.
const kept = new WeakMap();

function folded(row) {
  let one = kept.get(row);
  if (!one) {
    const title = fold(row.title);
    one = { title, text: title.letters.join(''), keywords: (row.keywords ?? []).map((keyword) => fold(keyword).letters) };
    kept.set(row, one);
  }
  return one;
}

/** Whether the row's facts pass each filter: a fact is a word or a list of words, and a filter's value is the start of one of them. */
function passes(row, filters) {
  return filters.every(({ key, value }) => {
    const fact = row.facts?.[key];
    return [].concat(fact ?? []).some((word) => foldedText(String(word)).startsWith(value));
  });
}

/** The score of the row for `words`, and the code points of its title that matched; null when a word is nowhere in it. */
function matchRow(one, words, text) {
  let score = 0;
  const marked = new Set();
  for (const word of words) {
    const inTitle = matchWord(word, one.title.letters);
    let best = inTitle;
    let marks = inTitle ? inTitle.places.flatMap((place) => codePointsOf(one.title, place)) : [];
    for (const keyword of one.keywords) {
      const found = matchWord(word, keyword);
      if (found && (best === null || Math.floor(found.score / 2) > best.score)) {
        best = { score: Math.floor(found.score / 2) };
        marks = [];
      }
    }
    if (best === null) return null;
    score += best.score;
    for (const mark of marks) marked.add(mark);
  }
  if (text && one.text.startsWith(text)) score += STARTS_TEXT;
  return { score, marks: [...marked].sort((a, b) => a - b) };
}

/** Where a row stands among the recent ones: 0 for the newest, Infinity for a row that is not recent. */
const recency = (row) => (typeof row.recent === 'number' ? row.recent : Number.POSITIVE_INFINITY);

/**
 * The rows that match `query`, best first, at most `limit`, as `{row,
 * score, marks}`. A row is `{title, keywords, facts, rank, recent}`:
 * `facts` maps a filter key to a word or a list of words; `rank` is 0 for
 * a page, 1 for an action and 2 for a thing; `recent` is the row's place
 * in the recent list, or false. `filterKeys` are the keys that `key:value`
 * words filter by. With nothing to match, every row that passes the filters
 * is given, the recent ones first and the others in the order they came.
 */
export function search(rows, query, { limit = MAX_ROWS, filterKeys = [] } = {}) {
  const { words, filters, text } = parseQuery(query, filterKeys);
  const found = [];
  rows.forEach((row, index) => {
    if (!passes(row, filters)) return;
    if (words.length === 0) {
      found.push({ row, score: 0, marks: [], index });
      return;
    }
    const match = matchRow(folded(row), words, text);
    if (match) found.push({ row, ...match, index });
  });
  const shorter = (one) => folded(one.row).title.letters.length;
  found.sort((a, b) => {
    if (words.length === 0) return recency(a.row) - recency(b.row) || a.index - b.index;
    return b.score - a.score
      || recency(a.row) - recency(b.row)
      || (a.row.rank ?? 0) - (b.row.rank ?? 0)
      || shorter(a) - shorter(b)
      || a.index - b.index;
  });
  return found.slice(0, limit).map(({ row, score, marks }) => ({ row, score, marks }));
}

/**
 * `title` cut into runs, each `{text, hit}`, where `hit` says that the run
 * is matched letters, whose code points are at `marks`.
 */
export function highlight(title, marks) {
  const hits = new Set(marks);
  const runs = [];
  let index = 0;
  for (const point of title) {
    const hit = hits.has(index);
    const last = runs.at(-1);
    if (last && last.hit === hit) last.text += point;
    else runs.push({ text: point, hit });
    index += 1;
  }
  return runs;
}
