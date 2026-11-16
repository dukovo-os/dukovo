// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The addresses of the Databases pages: the page of a database, and what the
// part of an address after /databases/ asks for. Kept apart from
// database-model.js so that the list, which is read first, does not read the
// rest of it.

/** The address of the page of the database `name`: one called `add` is written so that the page of Add a database is not taken for it. */
export const hrefOf = (name) => `/databases/${name === 'add' ? '%61dd' : encodeURIComponent(name)}`;

/**
 * What the part of the address after `/databases/` asks for: `{kind:
 * 'list'}`, `{kind: 'add'}` or `{kind: 'database', name}`.
 */
export function routeOf(rest) {
  const text = String(rest ?? '').replace(/\/+$/, '');
  if (text === '') return { kind: 'list' };
  if (text === 'add') return { kind: 'add' };
  try {
    return { kind: 'database', name: decodeURIComponent(text) };
  } catch {
    return { kind: 'database', name: text };
  }
}
