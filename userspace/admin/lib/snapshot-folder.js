// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The rows of a snapshot's folder (design 5.6), as plain values: how each
// entry compares with the same path now, which can be chosen for a restore,
// the folders down to this one, and the name that a restored copy gets.
// `snap browse --json` gives sizes in bytes. Nothing here touches the page.

import { bytesWords } from './size-words.js';

/** Whether `folder` is `place` or inside it, by whole names: `/users` holds `/users/alice` and not `/users2`. */
export const within = (place, folder) => folder === place || folder.startsWith(`${place}/`);

/**
 * The name that a restored copy of `path` gets beside it, as `snap restore
 * --beside` makes it: `PARENT/STEM (LABEL)EXT`, or `NAME (LABEL)` for a
 * folder and for a name with no extension. A name that starts with a dot has
 * no extension.
 */
export function besideName(path, label, { folder = false } = {}) {
  const slash = path.lastIndexOf('/');
  const parent = path.slice(0, slash + 1);
  const name = path.slice(slash + 1);
  const dot = name.lastIndexOf('.');
  if (folder || dot <= 0) return `${parent}${name} (${label})`;
  return `${parent}${name.slice(0, dot)} (${label})${name.slice(dot)}`;
}

/** How a folder's entry compares with the same path now: its words and the look of its pill. The same gives none. */
const THEN = Object.freeze({
  deleted: { word: 'deleted since', tone: 'warn' },
  changed: { word: 'changed since', tone: '' },
});

/** The most paths that one restore takes, as `snapshots.restore` says. */
export const RESTORE_MOST = 100;

/** The kinds of entry that a restore can copy: a path that holds a line break cannot be a line of `paths`. */
const restorable = (entry, kind) => kind !== 'other' && !/[\n\r\0]/.test(entry.name);

/**
 * The rows of a snapshot's folder from `snap browse --json`'s answer, in the
 * order the command gives (folders first, then by name): `{name, kind, path,
 * folder, size, then, pickable}`, `folder` whether it opens, `size` in words
 * (empty for a folder), `then` `{word, tone}` or null, `pickable` whether it
 * can be chosen for a restore.
 */
export function rowsOf(browse) {
  const base = String(browse?.folder ?? '').replace(/\/$/, '');
  return (Array.isArray(browse?.entries) ? browse.entries : []).filter((entry) => typeof entry?.name === 'string').map((entry) => {
    const kind = ['file', 'folder', 'link'].includes(entry.kind) ? entry.kind : 'other';
    return {
      name: entry.name,
      kind,
      path: `${base}/${entry.name}`,
      folder: kind === 'folder',
      size: kind === 'folder' || !Number.isFinite(entry.size) ? '' : bytesWords(entry.size),
      then: Object.hasOwn(THEN, entry.then) ? THEN[entry.then] : null,
      pickable: restorable(entry, kind),
    };
  });
}

/** The folder names of `folder` from the top of `place` down, each with the folder it opens: `[{name: '/users', folder: '/users'}, {name: 'owner', folder: '/users/owner'}]`. */
export function trail(place, folder) {
  const rest = within(place, folder) ? folder.slice(place.length).split('/').filter(Boolean) : [];
  return [{ name: place, folder: place }, ...rest.map((name, at) => ({ name, folder: `${place}/${rest.slice(0, at + 1).join('/')}` }))];
}
