// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Overview takes from the services (design 3.5 and 8.6): each
// service that gave up, and each change that Spark undoes unless someone
// keeps it, is a thing to look at. Both come from the one document that
// `GET /api/1/services` gives. A `href` is followed only where the page it
// names is registered.

import { needsOf, trialNeeds } from './needs.js';

/** The things to look at, for the Overview. */
export async function needs(cx) {
  const show = await cx.cache('/api/1/services', 5000, () => cx.api.get('/api/1/services'));
  const now = Math.floor(Date.now() / 1000);
  return [...needsOf(show, now), ...trialNeeds(show, now, cx.time)];
}
