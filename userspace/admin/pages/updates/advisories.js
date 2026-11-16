// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// "Show the advisories": every advisory that applies to the system and each
// flaw of an app installed, read from `update advisories --json` and drawn
// under the Security card.

import { busy, fill, h } from '../../dom.js';
import { fixWords } from './model.js';

const list = (value) => (Array.isArray(value) ? value : []);

/** The rows of `update advisories --json`: the system's, then each app's flaw, the worst first as the command gives them. */
export function advisoryRows(answer) {
  const row = (flaw, where, fix) => ({
    id: flaw.id, severity: flaw.severity ?? 'unknown', summary: flaw.summary ?? '', where, aliases: list(flaw.aliases), fix, action: typeof flaw.action === 'string' ? flaw.action : null,
  });
  return [
    ...list(answer?.system).map((flaw) => row(flaw, 'The system', fixWords(flaw))),
    ...list(answer?.apps).map((flaw) => row(flaw, flaw.item, flaw.fixed_by_update ? `${flaw.fixed_by_update} has the fix and is offered.` : 'No version of the app has the fix yet.')),
  ];
}

/** Reads the advisories and draws them in `box`; `one` is the button that was pressed. */
export async function showAdvisories(ctx, box, one) {
  try {
    const answer = await busy([one], () => ctx.view.api.get('/api/1/updates/advisories'));
    const rows = advisoryRows(answer);
    fill(box, rows.length === 0
      ? h('p', {}, 'No advisory applies and no flaw is known.')
      : h('ul', { class: 'upd-list' }, ...rows.map((row) => h('li', {},
        h('b', {}, row.id), ` · ${row.severity} · ${row.where}`,
        h('div', {}, row.summary),
        h('div', { class: 'muted' }, row.fix),
        row.action && h('div', { class: 'muted' }, row.action),
        row.aliases.length > 0 && h('div', { class: 'muted mono' }, row.aliases.join(', '))))));
  } catch (error) {
    ctx.view.problem(error, { slot: box });
  }
}
