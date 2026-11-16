// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the palette knows of databases: each database the page last read, and
// Add a database and Make a new password for each, for an administrator only.
// Listing the databases signs in to PostgreSQL as its superuser and leaves a
// line in the journal, so the palette never reads them: it has those this page
// read, which is none until it did. A new password is not a phrase that runs
// from the palette, since its answer would be shown nowhere; the row opens the
// database's page, which asks and shows it once. The palette reads this file
// when it opens.

import { hrefOf } from '../../lib/database-route.js';
import { bytesWords } from '../../lib/size-words.js';
import { remembered } from './seen.js';

/** The rows for `read` (the page's `readOf`, or null) and `session`. */
export function itemsOf(read, session) {
  if (session?.admin !== true) return [];
  const rows = [{ key: 'add', title: 'Add a database', detail: 'A database and the role that owns it', href: '/databases/add' }];
  for (const db of read?.databases ?? []) {
    const size = db.bytes === null ? '' : ` · ${bytesWords(db.bytes)}`;
    rows.push(
      { key: db.name, title: db.name, detail: `Owner ${db.owner}${size}${db.site ? ` · for ${db.site}` : ''}`, href: hrefOf(db.name) },
      { key: `password:${db.name}`, title: `Make a new password for ${db.name}`, detail: 'Asks first, then shows it once', href: `${hrefOf(db.name)}?password` },
    );
  }
  return rows;
}

export const providers = [{
  group: 'Databases',
  kind: 'database',
  filters: {},
  items: async (cx) => itemsOf(remembered(cx.session?.name), cx.session),
}];
