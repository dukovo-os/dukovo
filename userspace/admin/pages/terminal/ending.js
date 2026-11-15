// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Ending a pane or a terminal (design 13.10), which asks while a program
// runs in it, and taking off a pane that ended and stayed. The workspace
// reads this file when one of them is first asked for.

import { get, post } from '../../api.js';
import { withKept } from './workspace-model.js';

const API = '/api/1/terminals';

// Whether a program runs in one of `ids`, from a fresh list.
async function running(ids) {
  try {
    const { terminals } = await get(API);
    return terminals.some((one) => one.panes.some((pane) => ids.includes(pane.id) && pane.busy === true));
  } catch {
    return false;
  }
}

/** Ends `pane` of the workspace `ws`; a pane that ended already is taken off. */
export async function endPane(ws, pane) {
  if (pane.ended) {
    dismiss(ws, pane);
    return;
  }
  if (await running([pane.id]) && !(await ws.confirm('End this pane?', 'A program is running in it, and it ends too.', 'End the pane'))) return;
  try {
    await post(`${API}/panes/${pane.id}/end`, {});
  } catch (error) {
    ws.view?.problem(error);
  }
}

/** Ends the terminal of tab `terminal`, or takes the tab off when admind does not list it any more. */
export async function closeTab(ws, terminal) {
  if (ws.listed.some((one) => one.id === terminal.id)) {
    if (await running(terminal.panes.map((pane) => pane.id)) && !(await ws.confirm('End this terminal?', 'A program is running in it, and it ends too.', 'End the terminal'))) return;
    try {
      await post(`${API}/${terminal.id}/end`, {});
    } catch (error) {
      ws.view?.problem(error);
      return;
    }
  }
  ws.ghosts.delete(terminal.id);
  ws.later();
}

/** Takes a pane that ended and stayed off the page. */
export function dismiss(ws, pane) {
  const terminal = ws.ghosts.get(pane.terminal);
  terminal.panes = terminal.panes.filter((one) => one.id !== pane.id);
  if (terminal.panes.length === 0) ws.ghosts.delete(pane.terminal);
  ws.terminals = withKept(ws.listed, ws.ghosts);
  if (!ws.terminal) ws.current = ws.terminals[0]?.id ?? null;
  ws.sync();
  ws.draw();
}
