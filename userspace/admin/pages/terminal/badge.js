// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The count beside Sessions in the navigation (design 3.5): how many
// terminals the person has. The part gives nothing when there are none.

/** The badge for an answer of `GET /api/1/terminals`, or null. */
export function badgeOf(answer) {
  const count = Array.isArray(answer?.terminals) ? answer.terminals.length : 0;
  return count > 0 ? { count, label: 'running' } : null;
}

export async function count(cx) {
  return badgeOf(await cx.cache('/api/1/terminals', 5000, () => cx.api.get('/api/1/terminals')));
}
