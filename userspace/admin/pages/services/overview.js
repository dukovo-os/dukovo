// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Overview takes from the services (design 3.5): each service that
// gave up is a thing to look at. The page of the services is C4's; its
// address is in `href` already, and the Overview falls back to nothing while
// it is not registered.

import { needsOf } from './needs.js';

/** The things to look at, for the Overview. */
export async function needs(cx) {
  const show = await cx.cache('/api/1/services', 5000, () => cx.api.get('/api/1/services'));
  return needsOf(show, Math.floor(Date.now() / 1000));
}
