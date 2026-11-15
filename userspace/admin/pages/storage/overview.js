// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Overview takes from the storage (design 4.6): a volume that lost a
// disk, a disk that says it is failing, a count that rose, and a volume that
// is full within 30 days at its rate. The disks come from `GET
// /api/1/storage`, which anyone may read, and the rate from the 30 days of
// use; the part is left with the disks alone when the history cannot be read.

import { needsOf } from './model.js';

/** The things to look at, for the Overview. */
export async function needs(cx) {
  const [answer, usage] = await Promise.all([
    cx.cache('/api/1/storage', 5000, () => cx.api.get('/api/1/storage')),
    cx.cache('/api/1/storage/usage/30d', 5000, () => cx.api.get('/api/1/storage/usage/30d')).catch(() => null),
  ]);
  return needsOf(answer, usage);
}
