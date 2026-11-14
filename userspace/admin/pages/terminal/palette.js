// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the palette finds of the Terminal (design 3.5): "New terminal", "New
// administrator terminal" for an administrator, and each of the person's
// sessions by name.

/** The rows for an answer of `GET /api/1/terminals` and the session `session`. */
export function itemsOf(answer, session) {
  const terminals = Array.isArray(answer?.terminals) ? answer.terminals : [];
  const items = [{ key: 'new', title: 'New terminal', detail: 'A shell in a new tab', href: '/terminal?new' }];
  if (session?.admin) items.push({ key: 'new-admin', title: 'New administrator terminal', detail: 'A shell as root, after Unlock', href: '/terminal?new=admin' });
  for (const terminal of terminals) {
    const panes = Array.isArray(terminal.panes) ? terminal.panes.length : 0;
    items.push({
      key: `session-${terminal.id}`, title: String(terminal.name), detail: `Session, ${panes} ${panes === 1 ? 'pane' : 'panes'}`, href: `/terminal?open=${terminal.id}`,
    });
  }
  return items;
}

export const providers = [{
  group: 'Terminal',
  kind: 'terminal',
  items: async (cx) => itemsOf(await cx.cache('/api/1/terminals', 5000, () => cx.api.get('/api/1/terminals')), cx.session),
}];
