// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the palette knows of the apps: each app installed for everyone, with
// its version and what waits, to be filtered by `state:update`, `state:held`
// and `state:flawed`, and the changes that name an app. The palette reads
// this file when it opens.

import { paletteWords, rowsOf, stateWord } from './list-model.js';

/** The words of the states an app is found by, the flawed first. */
export const STATES = ['flawed', 'held', 'update'];

/** The rows of the apps in `list` (`GET /api/1/apps`), with what the last check `last` says of each; the flawed come first, as in the page. */
export function itemsOf(list, last) {
  return rowsOf(list, last).everyone.map((row) => ({
    key: row.id,
    title: row.name,
    detail: paletteWords(row),
    state: stateWord(row) ?? '',
    href: row.href,
    action: 'apps.show',
    args: { id: row.id },
  }));
}

export const providers = [{
  group: 'Apps',
  kind: 'app',
  filters: { state: STATES },
  items: async (cx) => {
    // The last check may fail without the apps being any the less known.
    const [list, last] = await Promise.all([
      cx.cache('/api/1/apps', 5000, () => cx.api.get('/api/1/apps')),
      cx.cache('/api/1/updates', 5000, () => cx.api.get('/api/1/updates')).catch(() => undefined),
    ]);
    return itemsOf(list, last);
  },
}];

// The app's ID is what each of these changes takes, so it is what the phrase names.
export const phrases = {
  'apps.update': 'Update {id}',
  'apps.hold': 'Hold {id}',
  'apps.rollback': 'Go back on {id}',
};
