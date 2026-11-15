// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the palette's result view shows of a read action's answer (design
// 11.4), as plain values: for a list, a table of its first 50 rows; for one
// thing, its facts; and for both the JSON in a fold.

import { labelOf } from '../lib/schema-model.js';

/** The most rows of a list the table shows. */
export const RESULT_ROWS = 50;

/** The most columns the table shows. */
export const MAX_COLUMNS = 6;

// The columns of a list that come first when it has them: what a row is
// called, then what it is doing.
const FIRST = ['name', 'id', 'title', 'state', 'status', 'kind', 'description', 'days_left', 'enabled', 'used_by'];

const isObject = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
const isScalar = (value) => ['string', 'number', 'boolean'].includes(typeof value);

/** A value as a table cell shows it: text, a number or a flag as words, a list of those joined; null for what a cell cannot hold (an object, nothing). */
export function cellOf(value) {
  if (isScalar(value)) return String(value);
  if (Array.isArray(value) && value.length > 0 && value.every(isScalar)) return value.join(', ');
  return null;
}

/**
 * The list in an answer: the answer when it is an array of objects, or its
 * property that is the longest such array. `{name, items}`, with `name`
 * null for the answer itself, or null when there is no list.
 */
export function listIn(answer) {
  const objects = (value) => Array.isArray(value) && value.length > 0 && value.every(isObject);
  if (objects(answer)) return { name: null, items: answer };
  if (!isObject(answer)) return null;
  let best = null;
  for (const [name, value] of Object.entries(answer)) {
    if (objects(value) && (best === null || value.length > best.items.length)) best = { name, items: value };
  }
  return best;
}

/** The columns of a table for `items`: those that hold something a cell can show, the well-known ones first, at most six. */
export function columnsOf(items) {
  const seen = [];
  for (const item of items) {
    for (const [key, value] of Object.entries(item)) {
      if (!seen.includes(key) && cellOf(value) !== null) seen.push(key);
    }
  }
  const first = FIRST.filter((key) => seen.includes(key));
  return [...first, ...seen.filter((key) => !first.includes(key))].slice(0, MAX_COLUMNS).map((key) => ({ key, label: labelOf(key) }));
}

/**
 * What to show of `answer`: `{kind, json, ...}` where `kind` is `table`
 * (`columns`, `rows` of cells, `total` and `shown` rows, `name` of the list),
 * `facts` (`facts` as `[label, value]`) or `text` (`text`, for what is not
 * JSON). `json` is the answer as indented text.
 */
export function describeAnswer(answer) {
  const json = typeof answer === 'string' ? answer : JSON.stringify(answer, null, 2) ?? '';
  if (typeof answer === 'string') return { kind: 'text', text: answer, json };
  const list = listIn(answer);
  if (list) {
    const shown = list.items.slice(0, RESULT_ROWS);
    const columns = columnsOf(shown);
    return {
      kind: 'table',
      name: list.name,
      columns,
      rows: shown.map((item) => columns.map((column) => cellOf(item[column.key]) ?? '')),
      total: list.items.length,
      shown: shown.length,
      json,
    };
  }
  const entries = isObject(answer) ? Object.entries(answer) : [];
  const facts = entries.map(([key, value]) => [labelOf(key), cellOf(value)]).filter(([, value]) => value !== null);
  return { kind: 'facts', facts, json };
}

/** The words above a table: "Services: 3", or "Services: the first 50 of 120"; "Rows" when the list has no name. */
export function tableWords({ name, total, shown }) {
  const label = name === null ? 'Rows' : labelOf(name);
  return total > shown ? `${label}: the first ${shown} of ${total}` : `${label}: ${total}`;
}
