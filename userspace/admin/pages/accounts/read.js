// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The reads of the Accounts pages that may fail without the page failing: a
// read that adds to what is shown (the sign-in rules, when each account was
// last active, the sessions route) gives null when the server cannot answer
// it, and the page says less. Leaving the page (an abort) is never a failure
// to swallow.

import { ApiError } from '../../api.js';

/** Waits for `promise`, and gives null when it fails for a reason that is not the page being left. */
export const soft = (promise) => promise.catch((error) => {
  if (error?.name === 'AbortError') throw error;
  return null;
});

/** Whether `error` says that the account asked for is not there (ACC-101, or a 404). */
export const noSuchAccount = (error) => error instanceof ApiError && (error.id === 'ACC-101' || error.status === 404);

/** The address of an account's own route `tail`: `/api/1/accounts/chase/sessions`. */
export const accountPath = (name, tail = '') => `/api/1/accounts/${encodeURIComponent(name)}${tail}`;
