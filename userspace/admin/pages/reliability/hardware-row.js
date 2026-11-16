// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Hardware checks row of the events table: what `hardware history` says
// of each day, as events of the kind the table counts, with a dot in the
// warning or the error colour for the worst result of the day, and a link to
// the Hardware page, which holds the checks themselves.

const counted = (count, one) => `${count} ${one}${count === 1 ? '' : 's'}`;

/** The row of the table: its kind and its title. */
export const HARDWARE_KIND = Object.freeze({ id: 'hardware', title: 'Hardware checks' });

/**
 * The checks of `history` (the lines `hardware history` gives, oldest first)
 * as events: `{kind, at, dot, title, sub, href, link, failed, warned}`, with
 * `at` in Unix milliseconds. A line with no time is left out.
 */
export function hardwareEvents(history) {
  return (Array.isArray(history) ? history : []).filter((line) => Number.isFinite(line?.at_ms)).map((line) => {
    const counts = [
      line.passed > 0 && `${line.passed} passed`,
      line.warnings > 0 && counted(line.warnings, 'warning'),
      line.failed > 0 && `${line.failed} failed`,
      line.skipped > 0 && `${line.skipped} skipped`,
    ].filter(Boolean);
    const ids = Array.isArray(line.ids) && line.ids.length > 0 ? ` · ${line.ids.join(', ')}` : '';
    return {
      kind: HARDWARE_KIND.id,
      at: line.at_ms,
      dot: line.failed > 0 ? 'bad' : line.warnings > 0 ? 'warn' : 'info',
      title: line.full ? 'Full hardware check' : 'Quick hardware check',
      sub: `${counts.join(', ') || 'Nothing could be checked'}${ids}`,
      href: '/hardware',
      link: 'Open the Hardware page',
      failed: line.failed > 0,
      warned: line.warnings > 0,
    };
  });
}

/** What one cell says of the checks of its day: `{count, dot, words}`, the dot that of the worst. */
export function hardwareCell(list) {
  const failed = list.filter((event) => event.failed).length;
  const warned = list.filter((event) => event.warned && !event.failed).length;
  const words = [counted(list.length, 'check'), failed > 0 && `${failed} failed`, warned > 0 && `${warned} with a warning`].filter(Boolean).join(', ');
  return { count: list.length, dot: failed > 0 ? 'bad' : warned > 0 ? 'warn' : 'info', words };
}
