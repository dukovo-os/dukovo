// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The kept result of dukovo-check, which the Overview's Health card and the
// Reliability page both read, and what each state of the read says.

import { assess } from './health.js';

/** What a card says when it has no score, by the state of the read. */
export const CHECK_NOTES = Object.freeze({
  missing: 'Checks are not on this server yet.',
  reader: 'The health score is for administrators.',
  none: 'Not checked yet.',
  error: 'The checks could not be read.',
});

/**
 * Where the kept result stands: `{state: 'ok', assessed}` with lib/health.js's
 * `assess`, or `state` one of `missing`, `reader`, `none` and `error`.
 * `GET /api/1/checks/last` answers 501 ADM-129 on a server without
 * `dukovo-check`, ADM-123 when no result is kept, and the result. A reader
 * does not ask: the result is for administrators.
 */
export async function readChecks(view) {
  if (!view.session.admin) return { state: 'reader' };
  try {
    const result = await view.api.get('/api/1/checks/last');
    if (!Array.isArray(result?.results)) return { state: 'none' };
    return { state: 'ok', assessed: assess(result) };
  } catch (error) {
    if (error?.name === 'AbortError') throw error;
    if (error?.id === 'ADM-129') return { state: 'missing' };
    if (error?.id === 'ADM-123') return { state: 'none' };
    return { state: 'error' };
  }
}
