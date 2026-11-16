// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the page that adds an app says before a sheet is open, as plain values
// so that a test reaches every branch: the words of a search and the rows it
// finds, the banners of catalogs that could not be read, and the ready-made
// setups with what each installs first. The sheet of one app, and what stands
// after its install, are in sheet-model.js, which the page reads only for a
// sheet. The words for flaws are those of the Apps pages.

import { flawWords, isText, listOf, urgent, worstOf } from '../pages/apps/apps-model.js';
import { initialOf, tintOf } from '../pages/apps/mark.js';
import { NGINX_ID, PHP_ID, POSTGRESQL_ID } from './known-bundles.js';

// ---- the search ---------------------------------------------------------------

/** The most words a search takes, and the shape of each: what `apps.search` checks. */
export const MAX_WORDS = 5;
const WORD = /^[a-z0-9.@-]{1,64}$/;

/**
 * What was typed in the search, as the words `apps.search` takes: lowercase,
 * split at spaces. Gives `{words, problem}`: `problem` says why the search
 * cannot be sent (more than five words, or a character no ID or name has), and
 * then `words` is empty.
 */
export function wordsOf(text) {
  const words = String(text ?? '').toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length > MAX_WORDS) return { words: [], problem: `Search for at most ${MAX_WORDS} words at a time.` };
  if (!words.every((word) => WORD.test(word))) return { words: [], problem: 'Words of a search are letters, digits, dots, @ and dashes.' };
  return { words, problem: null };
}

/** The address that reads the bundles the catalogs list for `words` (none: every bundle). */
export const searchPath = (words) => `/api/1/apps/catalog/search${words.length > 0 ? `?words=${encodeURIComponent(words.join(' '))}` : ''}`;

/** The address that reads the sheet of the app `id`. */
export const sheetPath = (id) => `/api/1/apps/catalog/${encodeURIComponent(id)}/sheet`;

/** The page that adds an app, with what is typed in its search kept in the address. */
export const addHref = (typed = '') => `/apps/add${typed === '' ? '' : `?q=${encodeURIComponent(typed)}`}`;

/** The address of the sheet of the app `id`: a dot is written out, as for an app's page. */
export const sheetHref = (id, typed = '') => `/apps/add/${encodeURIComponent(id).replaceAll('.', '%2E')}${typed === '' ? '' : `?q=${encodeURIComponent(typed)}`}`;

// ---- the rows of the catalogs ---------------------------------------------------

/** What the Flaws cell says of `flaws` (`{state, count, worst}`): the words, and whether they are in the warning colour. */
function flawsCell(flaws) {
  const known = flawWords(flaws);
  if (known !== null) return { words: known, urgent: urgent(worstOf([flaws.worst])) };
  if (flaws?.state === 'none' || flaws?.state === 'known') return { words: 'No known flaws', urgent: false };
  return { words: 'Flaws not listed', urgent: false };
}

/** Whether `a` comes before `b`: by name, as people read it, then by ID. */
const byName = (a, b) => {
  const [one, other] = [a.name.toLowerCase(), b.name.toLowerCase()];
  if (one !== other) return one < other ? -1 : 1;
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
};

/**
 * The rows of the table of the catalogs, from `apps.search`'s answer and the
 * apps that are installed (`apps.list`'s `bundles`), sorted by name:
 * `{id, name, version, catalog, href, initial, tint, flaws, installed}`.
 * `installed` is the version installed, from the search's own member or from
 * the list, else null; `typed` is kept in each row's address.
 */
export function rowsOf(answer, apps = [], typed = '') {
  const here = new Map(listOf(apps).filter((one) => isText(one?.id) && isText(one.version)).map((one) => [one.id, one.version]));
  return listOf(answer?.bundles).filter((one) => isText(one?.id)).map((one) => {
    const name = isText(one.name) ? one.name : one.id;
    return {
      id: one.id,
      name,
      version: isText(one.version) ? one.version : '',
      catalog: isText(one.catalog) ? one.catalog : '',
      href: sheetHref(one.id, typed),
      initial: initialOf(name),
      tint: tintOf(one.id),
      flaws: flawsCell(one.flaws),
      installed: isText(one.installed) ? one.installed : here.get(one.id) ?? null,
    };
  }).sort(byName);
}

/** What a catalog's reason says when the network is why: the Updates page is where a connection is looked at. */
const NETWORK = /network|connect|resolve|dns|timed? ?out|unreachable|offline|route/i;

/**
 * A banner for each catalog that could not be read, `{catalog, words,
 * network}`: `network` is true when the reason is the network, and the
 * banner then links to the Updates page. "The list shows the others." is
 * said only when the list has an app.
 */
export function unreadOf(answer) {
  const others = listOf(answer?.bundles).length > 0;
  return listOf(answer?.unread).filter((one) => isText(one?.catalog)).map((one) => {
    const why = isText(one.why) ? one.why.replace(/[.\s]+$/, '') : 'no reason was given';
    return {
      catalog: one.catalog,
      words: `The catalog ${one.catalog} could not be read: ${why}.${others ? ' The list shows the others.' : ''}`,
      network: NETWORK.test(why),
    };
  });
}

/**
 * What the table says when it has no row, as `{kind, words}`: no catalog is
 * on (no word was typed and nothing is listed, nothing unread), nothing
 * matches, or no catalog could be read. Null when there are rows.
 */
export function emptyOf(answer, words) {
  if (listOf(answer?.bundles).length > 0) return null;
  if (listOf(answer?.unread).length > 0) return { kind: 'unread', words: 'No app is listed: no catalog could be read.' };
  if (words.length === 0) return { kind: 'none', words: 'No catalog is turned on.', command: 'bundle catalog' };
  return { kind: 'nomatch', words: `No app in the catalogs matches “${words.join(' ')}”.` };
}

// ---- the ready-made setups ------------------------------------------------------

const NGINX = { id: NGINX_ID, name: 'nginx' };
const PHP = { id: PHP_ID, name: 'PHP-FPM' };
const POSTGRESQL = { id: POSTGRESQL_ID, name: 'PostgreSQL' };

/** The setups the machine does, each a link to the page that does it, with the apps it uses. */
const SETUPS = Object.freeze([
  {
    id: 'static', title: 'Static website', words: 'Pages and files from a folder, with HTTPS.', uses: [NGINX], href: '/websites/add?kind=static',
  },
  {
    id: 'php', title: 'PHP website with a database', words: 'For your own PHP code or a PHP app, with HTTPS.', uses: [NGINX, PHP, POSTGRESQL], href: '/websites/add?kind=php&database=1',
  },
  {
    id: 'database', title: 'A database', words: 'A database and its role for your own apps, with a password shown once.', uses: [POSTGRESQL], href: '/databases/add',
  },
  {
    id: 'private', title: 'Private access for your devices', words: 'Reach this server from your laptop and phone, from anywhere.', uses: [{ id: null, name: 'WireGuard' }], href: '/security#private',
  },
]);

/** "nginx", "nginx and PostgreSQL", "nginx, PHP-FPM and PostgreSQL". */
const listWords = (names) => (names.length < 2 ? names.join('') : `${names.slice(0, -1).join(', ')} and ${names.at(-1)}`);

/**
 * The setups that `text` matches (each word of it, in a title or a name of
 * what it uses), with what each needs installed first: `{id, title, words,
 * href, uses: [name], installs: [name], says}`. `says` is "Installs nginx
 * first" when an app is missing, else null. `installedIds` is the IDs of the
 * apps that are installed, or null when they could not be read, and then no
 * setup says what it installs.
 */
export function setupsOf(text, installedIds) {
  const have = new Set(installedIds ?? []);
  const words = String(text ?? '').toLowerCase().split(/\s+/).filter(Boolean);
  return SETUPS.filter((setup) => {
    const hay = [setup.title, ...setup.uses.map((one) => one.name)].join(' ').toLowerCase();
    return words.every((word) => hay.includes(word));
  }).map((setup) => {
    const missing = installedIds === null ? [] : setup.uses.filter((one) => one.id !== null && !have.has(one.id)).map((one) => one.name);
    return {
      id: setup.id,
      title: setup.title,
      words: setup.words,
      href: setup.href,
      uses: setup.uses.map((one) => one.name),
      installs: missing,
      says: missing.length > 0 ? `Installs ${listWords(missing)} first` : null,
    };
  });
}
