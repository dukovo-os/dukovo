// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Overview's own part (design 3.5): the tasks of the last 24 hours that
// failed or were stopped, and the hand edits of the last 7 days that nobody
// has marked as seen; and the steps that Finish setting up and /setup show.

import { EDIT_LIMIT, EDIT_SECONDS, editNeeds, taskNeeds } from './part-model.js';

export { steps } from './steps.js';

/** The things to look at, for the Overview. */
export async function needs(cx) {
  const now = Math.floor(Date.now() / 1000);
  const [tasks, changes] = await Promise.all([
    cx.cache('/api/1/tasks', 5000, () => cx.api.get('/api/1/tasks')),
    cx.cache('changes-week', 5000, () => cx.api.get(`/api/1/changes?since=${now - EDIT_SECONDS}&limit=${EDIT_LIMIT}`)),
  ]);
  return [...taskNeeds(tasks, now), ...editNeeds(changes, now)];
}
