// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Overview takes from the certificates (design 3.5): one that was
// revoked, has ended or has 14 days or fewer left. The page of the
// certificates is C6's; its address is in `href` already.

import { needsOf } from './needs.js';

/** The things to look at, for the Overview. */
export async function needs(cx) {
  return needsOf(await cx.cache('/api/1/certificates', 5000, () => cx.api.get('/api/1/certificates')));
}
