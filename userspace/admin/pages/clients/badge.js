// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The count beside "AI clients" in the navigation: the changes that clients
// asked for and that wait for a yes. It is read when the console starts,
// and again at once when the stream's `clients` event says that a request
// moved, since a request that waits is waited on.

import { badgeOf } from './model.js';

/** The badge, or null when nothing waits or the account is no administrator's. */
export async function count(cx) {
  if (cx.session.admin !== true) return null;
  const answer = await cx.api.get('/api/1/clients/requests');
  return badgeOf(Array.isArray(answer?.requests) ? answer.requests.length : 0);
}
