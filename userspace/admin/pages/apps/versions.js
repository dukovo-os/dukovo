// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The table of an app's versions: the one offered, the one installed and the
// one kept for going back. Older versions are not kept, and the table says
// so when none is. Go back to this is on the one that is kept.

import { h } from '../../dom.js';
import { NO_PREVIOUS, versionsOf } from './app-model.js';
import { onPress } from './list.js';

/**
 * The table for the app of `info` (`bundle info --json`) and its `update`
 * (the row's). `admin` says whether the person may go back; `back(button)`
 * is called when Go back to this is pressed.
 */
export function versionsTable(view, {
  info, update, admin, back,
}) {
  const rows = versionsOf(info, update, view.time);
  const go = (row) => {
    if (!row.back || !admin) return null;
    const button = h('button', { class: 'btn small', type: 'button', 'data-action': 'back', 'data-guard': '' }, 'Go back to this');
    onPress(button, back);
    return h('div', { class: 'apps-action' }, button, view.api.inline('apps.rollback', { id: info.id }));
  };
  return h('div', { class: 'apps-versions' },
    h('div', { class: 'tbl-box' },
      h('table', { class: 'tbl' },
        h('caption', { class: 'sr-only' }, `Versions of ${info.name}`),
        h('tbody', {}, rows.map((row) => h('tr', {},
          h('td', {}, h('b', {}, row.version), row.note && h('small', {}, row.note)),
          h('td', { class: 'end' }, row.pill && h('span', { class: `pill ${row.pill.kind}` }, row.pill.words), go(row))))))),
    !info.previous && h('p', { class: 'small muted' }, NO_PREVIOUS));
}
