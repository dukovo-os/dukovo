// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the palette finds (design 11.1), as plain values: one list of rows
// built from the pages, the actions admind lists and the things the pages'
// palette parts give, and what to show of it for a query: the rows by
// group, with the pills each row wears. The matching is lib/fuzzy.js's; the
// dialog is palette.js.
//
// A row is `{key, kind, group, title, detail, keywords, facts, rank, icon,
// state, target, change, elevated, recent}`:
//
// - `kind` is `page`, `action`, `thing` or `phrase` (an action made of a
//   thing: "Restart nginx"); `rank` is 0 for a page, 1 for an action or a
//   phrase and 2 for a thing, which breaks a tie in that order;
// - `facts` are what `key:value` words filter by: `kind` and, for a thing,
//   what its provider says (`state`);
// - `target` is where Enter goes: `{path}` for a page, `{action, args}` for
//   an action, or null when the row has nowhere to go yet;
// - `recent` is the row's place in the recent list, or false.

import { lowerFirst, needsAdministrator } from '../lib/action-model.js';
import { MAX_ROWS, parseQuery, search } from '../lib/fuzzy.js';
import { checkArgs, formFor, isReady } from '../lib/schema-model.js';
import { matchPage } from '../shell/route-model.js';

export const RECENT_GROUP = 'Recent';
export const ACTIONS_GROUP = 'Actions';
export const PAGES_GROUP = 'Settings and pages';

/** How many things the recent list keeps. */
export const RECENT_MAX = 8;

// A page that is shown once, to be left for good: nothing to find in it.
const NOT_FOUND = new Set(['welcome']);

// The icon of an action by its area, for the areas admind has.
const AREA_ICONS = {
  services: 'services', certificates: 'certs', security: 'security', console: 'info', metrics: 'reliability',
  reliability: 'reliability', logs: 'logs', network: 'network', firewall: 'firewall', storage: 'storage',
  snapshots: 'snapshots', backups: 'snapshots', move: 'server', accounts: 'accounts',
};

// How a state word is drawn: the board's `st` tones.
const TONES = {
  failed: 'bad', expired: 'bad', revoked: 'bad', error: 'bad',
  expiring: 'warn', degraded: 'warn', warning: 'warn', waiting: 'warn', stopping: 'warn',
  running: 'ok', active: 'ok', valid: 'ok', ok: 'ok', healthy: 'ok', done: 'ok',
};

const isText = (value) => typeof value === 'string' && value !== '';
const isObject = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);

/** The name the recent list is kept under in localStorage for this account (`lib/store.js` adds the prefix). */
export const recentName = (account) => `recent.v1.${account}`;

/** What was kept as the recent list, made safe: at most eight keys of text. */
export const cleanRecent = (kept) => (Array.isArray(kept) ? kept.filter(isText).slice(0, RECENT_MAX) : []);

/** The recent list with `key` as its newest. */
export const remember = (list, key) => [key, ...list.filter((one) => one !== key)].slice(0, RECENT_MAX);

/** Marks each row of `rows` with its place in `list` of keys (0 is the newest), or false. */
export function applyRecent(rows, list) {
  for (const row of rows) {
    const place = list.indexOf(row.key);
    row.recent = place === -1 ? false : place;
  }
}

/** Whether `entry` may be shown to `session` and found: a page that is registered, that the person may see and that is not a stand-in for a moment. */
function mayShow(entry, session) {
  if (entry.load === null || entry.load === undefined || NOT_FOUND.has(entry.id)) return false;
  if (entry.needs === 'admin' && !session.admin) return false;
  if (entry.nav?.flag && !session[entry.nav.flag]) return false;
  return true;
}

/** Whether `href` is the address of a page that is registered and that `session` may see. */
export function reaches(pages, session, href) {
  if (!isText(href)) return false;
  const found = matchPage(pages, href.split(/[?#]/)[0]);
  return found !== null && mayShow(found.entry, session);
}

const row = (fields) => ({
  detail: '', keywords: [], state: null, target: null, change: false, elevated: false, recent: false, ...fields,
});

/** A row for each page `session` may see, and for its children and sections ("Terminal › Sessions"). */
export function pageRows(pages, session) {
  const rows = [];
  for (const entry of pages.filter((one) => mayShow(one, session))) {
    const own = (title, path) => row({
      key: `page:${path}`,
      kind: 'page',
      group: PAGES_GROUP,
      title,
      keywords: entry.keywords ?? [],
      facts: { kind: ['page'] },
      rank: 0,
      icon: entry.icon ?? 'code',
      target: { path },
    });
    rows.push(own(entry.title, entry.path));
    for (const child of [...(entry.children ?? []), ...(entry.sections ?? [])]) {
      rows.push(own(`${entry.title} › ${child.title}`, child.path));
    }
  }
  return rows;
}

/** A row for each action of `GET /api/1/actions`, to be run with arguments from its form. */
export function actionRows(actions) {
  return actions.filter((entry) => isText(entry?.id) && isText(entry.title)).map((entry) => row({
    key: `action:${entry.id}`,
    kind: 'action',
    group: ACTIONS_GROUP,
    title: entry.title,
    keywords: [entry.area, entry.id.replace(/[._-]+/g, ' ')].filter(isText),
    facts: { kind: ['action'] },
    rank: 1,
    icon: AREA_ICONS[entry.area] ?? 'code',
    target: { action: entry.id, args: {} },
    change: entry.changes === true,
    elevated: needsAdministrator(entry),
  }));
}

/** The words of a state, as the row shows it, and the board's tone for it. */
export function stateOf(word) {
  if (!isText(word)) return null;
  return { word, label: word[0].toUpperCase() + word.slice(1), tone: TONES[word.toLowerCase()] ?? 'off' };
}

/** Whether the action `entry` takes `args` by its own schema: "Restart admind" is no row, since the action refuses the console's own service. An action that lists no schema takes them. */
function takes(entry, args) {
  const form = formFor(entry.input);
  return !form.ok || checkArgs(form, args).length === 0;
}

/** `phrase` with each `{name}` made the argument of that name, or null when one is missing. */
export function phraseFor(phrase, args) {
  let missing = false;
  const title = phrase.replace(/\{([^}]+)\}/g, (_, name) => {
    if (args[name] === undefined || args[name] === null) missing = true;
    return String(args[name]);
  });
  return missing ? null : title;
}

/**
 * The rows of what the pages' palette parts gave. `collected` is
 * `[{entry, value}]` as `parts.collect` gives it, where `value` is
 * `{phrases, groups}` and each of `groups` is `{group, kind, filters,
 * items}` (a provider with its items read). A thing is a row of its own;
 * each phrase of the part makes one more row for each thing that has
 * `args`, when the action it names is one of `actions` and takes them. A thing that has
 * an `href` to a page that is not registered goes to its `action` instead,
 * or nowhere.
 */
export function thingRows({ collected, actions, pages, session }) {
  const rows = [];
  const groups = [];
  const filters = {};
  const kinds = [];
  for (const { entry, value } of collected) {
    const phrases = isObject(value?.phrases) ? Object.entries(value.phrases).filter(([, words]) => isText(words)) : [];
    for (const provider of (Array.isArray(value?.groups) ? value.groups : []).filter(isObject)) {
      const group = isText(provider.group) ? provider.group : entry.title;
      const kind = isText(provider.kind) ? provider.kind : entry.id;
      if (!groups.includes(group)) groups.push(group);
      if (!kinds.includes(kind)) kinds.push(kind);
      for (const [name, words] of Object.entries(isObject(provider.filters) ? provider.filters : {})) {
        filters[name] = [...new Set([...(filters[name] ?? []), ...[].concat(words).filter(isText)])];
      }
      const seen = new Set();
      for (const item of Array.isArray(provider.items) ? provider.items : []) {
        if (!isText(item?.key) || !isText(item.title) || seen.has(item.key)) continue;
        seen.add(item.key);
        const args = isObject(item.args) ? item.args : null;
        const state = stateOf(item.state);
        const facts = { kind: [kind], ...(state ? { state: [state.word] } : {}) };
        const where = reaches(pages, session, item.href)
          ? { path: item.href }
          : (isText(item.action) ? { action: item.action, args: args ?? {} } : null);
        const goes = where?.action ? actions.find((one) => one?.id === where.action) : null;
        rows.push(row({
          key: `thing:${kind}:${item.key}`,
          kind: 'thing',
          group,
          title: item.title,
          detail: isText(item.detail) ? item.detail : '',
          keywords: isText(item.detail) ? [item.detail] : [],
          facts,
          rank: 2,
          icon: entry.icon ?? 'code',
          state,
          target: where,
          change: goes?.changes === true,
          elevated: goes ? needsAdministrator(goes) : false,
        }));
        if (args === null) continue;
        for (const [id, words] of phrases) {
          const action = actions.find((one) => one?.id === id);
          const title = action && takes(action, args) ? phraseFor(words, args) : null;
          if (title === null) continue;
          rows.push(row({
            key: `phrase:${id}:${kind}:${item.key}`,
            kind: 'phrase',
            group: ACTIONS_GROUP,
            title,
            keywords: isText(item.detail) ? [item.detail] : [],
            facts: { ...facts, kind: ['action'] },
            rank: 1,
            icon: AREA_ICONS[action.area] ?? entry.icon ?? 'code',
            target: { action: id, args },
            change: action.changes === true,
            elevated: needsAdministrator(action),
          }));
        }
      }
    }
  }
  return { rows, groups, filters, kinds };
}

/**
 * The index: `rows`, the `groups` in the order they are shown, the
 * `filterKeys` that `key:value` words filter by, the values each key
 * takes (`filters`), and `hints`, two filters for the foot to name.
 */
export function buildIndex({ pages, actions, collected, session }) {
  const things = thingRows({ collected, actions, pages, session });
  const filters = { kind: ['page', 'action', ...things.kinds], ...things.filters };
  const first = Object.entries(things.filters).find(([, words]) => words.length > 0);
  return {
    rows: [...pageRows(pages, session), ...actionRows(actions), ...things.rows],
    groups: [ACTIONS_GROUP, ...things.groups.filter((one) => one !== ACTIONS_GROUP && one !== PAGES_GROUP), PAGES_GROUP],
    filterKeys: Object.keys(filters),
    filters,
    hints: [`kind:${things.kinds[0] ?? 'page'}`, first ? `${first[0]}:${first[1][0]}` : null].filter(Boolean),
  };
}

/**
 * What `query` finds in `index`, best first, at most `limit`: `{row,
 * score, marks}`. A phrase is a row only once something is typed that is
 * not a filter, or when it is in the recent list, so that a list of every
 * service does not bring five rows for each.
 */
export function find(index, query, limit = MAX_ROWS) {
  const { words } = parseQuery(query, index.filterKeys);
  const rows = words.length > 0 ? index.rows : index.rows.filter((one) => one.kind !== 'phrase' || one.recent !== false);
  return search(rows, query, { limit, filterKeys: index.filterKeys });
}

/**
 * `results` of `find`, cut into groups in the order of `index.groups`, as
 * `[{label, results}]` with no empty group. With nothing typed the rows in
 * the recent list come first, in a group of their own, in the order they
 * were used.
 */
export function groupResults(index, results, query) {
  const empty = query.trim() === '';
  const recent = empty ? results.filter((one) => one.row.recent !== false) : [];
  const rest = empty ? results.filter((one) => one.row.recent === false) : results;
  const groups = [{ label: RECENT_GROUP, results: recent }];
  for (const label of index.groups) groups.push({ label, results: rest.filter((one) => one.row.group === label) });
  return groups.filter((one) => one.results.length > 0);
}

/** The rows of `groups` in the order the arrow keys walk them. */
export const flatten = (groups) => groups.flatMap((group) => group.results);

/** The pills a row wears for `who`, `{admin, unlocked}`: "Needs Unlock" for an administrator's change while the console is locked, "Administrators only" for a reader. */
export function pillsFor(one, { admin, unlocked }) {
  if (!one.elevated) return [];
  if (!admin) return [{ id: 'admin', label: 'Administrators only', tone: '' }];
  return unlocked ? [] : [{ id: 'unlock', label: 'Needs Unlock', tone: 'warn' }];
}

/** The words for the status line after the list changes: "12 results". */
export function countWords(count) {
  if (count === 0) return 'No results';
  return `${count} ${count === 1 ? 'result' : 'results'}`;
}

/** The words that `lib/action.js` says on the way for a row run from the palette: its own title, so that "Restart nginx" is what the unlock dialog and the notice say. */
export const runWords = (title) => ({
  unlock: lowerFirst(title),
  running: title,
  done: `${title}: done.`,
  unchanged: `${title}: nothing changed.`,
  busy: `${title}…`,
});

/** Whether `args` are all that the action `entry` needs by its schema; false when it lists no schema this console can read. */
export function argsReady(entry, args) {
  const form = formFor(entry?.input);
  return form.ok && isReady(form, args);
}
