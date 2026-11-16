// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The square that stands for an app: the first letter of its name, in one of
// six colours that its ID picks. The installed apps and the apps of the
// catalogs are marked the same way. The page names the class that its own
// sheet gives the square, so that no page carries the rules of another's.

import { h } from '../../dom.js';

/** The letter in the square that stands for an app: the first of its name. */
export const initialOf = (name) => (Array.from(String(name).trim())[0] ?? '?').toUpperCase();

/** The colour of that square, one of six that an app's ID picks, the same every time. */
export function tintOf(id) {
  let hash = 0;
  for (const letter of id) hash = (Math.imul(hash, 31) + letter.codePointAt(0)) >>> 0;
  return hash % 6;
}

/** The square with an app's initial, which stands for the app and is not read: its name is beside it. `row` is `{initial, tint}`; `kind` is the class that the page's sheet draws it with. */
export const square = (row, kind, big = false) => h('span', { class: `${kind} tint-${row.tint}${big ? ' big' : ''}`, 'aria-hidden': 'true' }, row.initial);
