// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Databases: /databases is the list, /databases/add makes one and
// /databases/NAME is one database's page. A database called add has its page at
// /databases/%61dd. Each is read when it is asked for.

import { routeOf } from '../../lib/database-route.js';
import { showList } from './list.js';

export async function show(view) {
  const route = routeOf(view.rest);
  if (route.kind === 'add') {
    await (await import('./add.js')).showAdd(view);
  } else if (route.kind === 'database') {
    await (await import('./database.js')).showDatabase(view, route.name);
  } else {
    await showList(view);
  }
}
