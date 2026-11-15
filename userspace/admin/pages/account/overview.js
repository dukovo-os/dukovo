// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Overview's need from the account (design 3.5): an administrator whose
// account has no second step is told to add one. The account's own list is
// read, not the session's `kinds`, which leaves out passkeys when the
// console is opened by the server's address.

import { needsOf } from './needs.js';

/** The things to look at, for the Overview. */
export async function needs(cx) {
  if (!cx.session.admin) return [];
  return needsOf(cx.session, await cx.cache('/api/1/account/second-step', 5000, () => cx.api.get('/api/1/account/second-step')));
}
