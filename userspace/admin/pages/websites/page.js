// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Websites: /websites is the list, /websites/add makes one and /websites/NAME
// is one site's page. A site called add has its page at /websites/%61dd. Each
// is read when it is asked for.

import { routeOf } from '../../lib/website-route.js';
import { showList } from './list.js';

export async function show(view) {
  const route = routeOf(view.rest);
  if (route.kind === 'add') {
    await (await import('./add.js')).showAdd(view);
  } else if (route.kind === 'site') {
    await (await import('./site.js')).showSite(view, route.name);
  } else {
    await showList(view);
  }
}
