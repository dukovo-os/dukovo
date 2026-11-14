// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The rows of the Sessions page (design 13.10): what admind lists of each
// terminal, in the words of the table. `time` is the console's (lib/time.js);
// `mine` is the set of panes this browser tab has on its socket.

/** Whether a terminal is open "in this tab", "in another tab", both, or "not open". */
export function openWords(terminal, mine) {
  const here = terminal.panes.some((pane) => mine.has(pane.id));
  // A pane that this tab shows is counted once among the sockets that show it.
  const elsewhere = terminal.panes.some((pane) => pane.attached > (mine.has(pane.id) ? 1 : 0));
  if (here && elsewhere) return 'in this tab and another';
  if (here) return 'in this tab';
  return elsewhere ? 'in another tab' : 'not open';
}

/** Whether a program runs in the terminal: "Yes", "No", "1 of 2 panes" or "Not known" when admind cannot tell. */
export function runningWords(terminal) {
  const known = terminal.panes.filter((pane) => typeof pane.busy === 'boolean');
  if (known.length === 0) return 'Not known';
  const busy = known.filter((pane) => pane.busy).length;
  if (busy === 0) return 'No';
  return busy === terminal.panes.length ? 'Yes' : `${busy} of ${terminal.panes.length} panes`;
}

/** Whether ending the terminal asks first: a program runs in it. */
export const asksToEnd = (terminal) => terminal.panes.some((pane) => pane.busy === true);

/** The table's rows, in the order admind lists them (the one used last first). */
export function rowsOf(terminals, { time, mine = new Set() }) {
  return [...terminals].sort((a, b) => b.used - a.used).map((terminal) => ({
    id: terminal.id,
    name: terminal.name,
    panes: terminal.panes.length,
    admin: terminal.panes.some((pane) => pane.admin),
    made: time.when(terminal.made),
    madeFull: time.full(terminal.made),
    used: time.relative(terminal.used),
    usedFull: time.full(terminal.used),
    from: terminal.from,
    open: openWords(terminal, mine),
    running: runningWords(terminal),
    asks: asksToEnd(terminal),
  }));
}
