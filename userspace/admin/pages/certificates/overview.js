// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Overview takes from the certificates: one that was revoked, has
// ended or has 14 days or fewer left. Each links to its certificate.

import { needsOf } from './needs.js';

/** The things to look at, for the Overview. */
export async function needs(cx) {
  return needsOf(await cx.cache('/api/1/certificates', 5000, () => cx.api.get('/api/1/certificates')));
}
