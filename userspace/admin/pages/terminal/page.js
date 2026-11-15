// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Terminal (design 13.10): /terminal is the workspace, and Sessions and
// Settings are read when they are asked for, so that nothing of them loads
// with the terminal. The workspace is kept for a while after the person
// leaves, so that going to Sessions and back does not attach every pane
// again, and Sessions can tell which terminals this tab has open.

import { Workspace } from './workspace.js';

const GRACE_MS = 30_000;
let parked = null;

export async function show(view) {
  if (view.rest === 'sessions') return (await import('./sessions.js')).show(view);
  if (view.rest === 'settings') return (await import('./settings.js')).show(view);
  if (view.rest !== '') {
    view.go('/terminal');
    return undefined;
  }
  view.head({ title: 'Terminal' });
  let workspace = parked?.workspace;
  if (parked) clearTimeout(parked.timer);
  parked = null;
  workspace ??= new Workspace(view.session);
  view.signal.addEventListener('abort', () => {
    workspace.unmount();
    parked = { workspace, timer: setTimeout(() => { workspace.dispose(); parked = null; }, GRACE_MS) };
  }, { once: true });
  await workspace.mount(view);
  const { query } = view;
  const open = Number(query.get('open') ?? query.get('t'));
  if (query.has('new')) {
    history.replaceState(null, '', '/terminal');
    await workspace.newTerminal({ admin: query.get('new') === 'admin' });
  } else if (open > 0 && workspace.terminals.some((one) => one.id === open)) {
    workspace.select(open);
  } else if (workspace.terminals.length === 0) {
    await workspace.newTerminal();
  }
  return undefined;
}
