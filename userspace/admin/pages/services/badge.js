// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The count beside "Services" in the navigation (design 3.5): the services
// that gave up. The navigation shows it once the page has an entry with `nav`.

import { badgeOf } from './needs.js';

/** The badge, or null when no service failed. */
export async function count(cx) {
  return badgeOf(await cx.cache('/api/1/services', 5000, () => cx.api.get('/api/1/services')));
}
