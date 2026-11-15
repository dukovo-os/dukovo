// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Terminal page works out from the terminals admind lists (design
// 13.10), as plain values: which panes to keep on the socket, how a tab and
// an administrator's pane are named, which tab comes next, and why a pane
// ended in words.

export const LAYOUTS = ['one', 'columns', 'rows', 'main-left', 'grid'];
export const LAYOUT_WORDS = {
  one: 'One pane', columns: 'Columns', rows: 'Rows', 'main-left': 'Main pane at the left', grid: 'Grid',
};

/** The most panes one socket may show (admind's limit). */
export const ATTACH_MOST = 16;

/**
 * The panes to attach, most wanted first: those of the terminal `current`,
 * then those of the others, most recently used terminal first, until `most`.
 */
export function attachPlan(terminals, current, most = ATTACH_MOST) {
  const ordered = [...terminals].sort((a, b) => (b.id === current) - (a.id === current) || b.used - a.used || a.id - b.id);
  return ordered.flatMap((terminal) => terminal.panes.map((pane) => pane.id)).slice(0, most);
}

/** A tab's name for a screen reader: "alice@duke-nas, administrator, new output". */
export function tabName({ name, admin = false, activity = false }) {
  return [name, admin && 'administrator', activity && 'new output'].filter(Boolean).join(', ');
}

/** What an administrator pane's head says: when it ends, if the minutes left are known. */
export function adminWords(minutes) {
  const ends = 'Administrator \u00b7 ends when the console locks';
  return minutes === null || minutes === undefined ? ends : `${ends}, in ${minutes} min`;
}

/** With this many minutes left of the unlock, an administrator pane offers Reset to 15 min (design 13.4). */
export const SOON_MINUTES = 2;

/** Whether an administrator pane offers to reset the unlock: it runs out in `minutes` or less. */
export const nearsEnd = (minutes) => minutes !== null && minutes !== undefined && minutes <= SOON_MINUTES;

/** The terminal after (`step` 1) or before (-1) `id` in `list`, round the end; the first when `id` is not there. */
export function tabAfter(list, id, step) {
  if (list.length === 0) return null;
  const at = list.findIndex((one) => one.id === id);
  return list[at < 0 ? 0 : (at + step + list.length) % list.length].id;
}

/** Whether a pane that ended for `why` goes at once: the program ended, or someone ended the pane. */
export const closesAtOnce = (why) => why === 'exited' || why === 'ended';

/** Why a pane ended, in words; `status` is the program's exit status when admind knows it. */
export function endedWords(why, status) {
  switch (why) {
    case 'exited':
      return status === undefined ? 'The program in this pane ended.' : `The program in this pane ended with status ${status}.`;
    case 'locked':
      return 'The console locked, which ends administrator panes.';
    case 'signed-out':
      return 'You signed out, which ends your terminals.';
    case 'idle':
      return 'No browser opened this terminal for the days it is kept, so it ended.';
    case 'restart':
      return 'The console restarted, which ends every terminal.';
    default:
      return 'This pane was ended.';
  }
}

/**
 * The terminals to show: those admind lists, and the ones that have a pane
 * that ended and stays (`kept`, a Map of terminal id to the terminal as it
 * was) so that the person reads why. A kept terminal holds the panes that
 * were kept, and any admind still lists.
 */
export function withKept(listed, kept) {
  const ids = new Set(listed.map((terminal) => terminal.id));
  const gone = [...kept.values()].filter((terminal) => !ids.has(terminal.id));
  const merged = listed.map((terminal) => {
    const old = kept.get(terminal.id);
    return old ? { ...terminal, panes: [...terminal.panes, ...old.panes.filter((pane) => !terminal.panes.some((one) => one.id === pane.id))] } : terminal;
  });
  return [...merged, ...gone];
}

/** The size of a pane's room as columns and rows, from the room in pixels and a cell's size, kept within what a screen may be. */
export function fitOf(width, height, cell) {
  const clamp = (n, low, high) => Math.min(Math.max(n, low), high);
  return { cols: clamp(Math.floor(width / cell.w), 2, 500), rows: clamp(Math.floor(height / cell.h), 1, 300) };
}

/** The text size one step (2 pixels) bigger (`step` 1) or smaller (-1), within 12 to 20, or null at the end. */
export function sizeStep(size, step) {
  const next = Math.min(Math.max(size + step * 2, 12), 20);
  return next === size ? null : next;
}
