// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Screens card says, as plain values: the graphics cards of
// `hardware screens` and each screen of them, with the monitor plugged in,
// the mode it shows and the modes it offers, and which program has the
// screens. What a monitor does not say is not said here either.

const DASH = '—';

/** A refresh rate of frames in 1000 seconds, in Hz, with decimals only where it has them. */
const rateOf = (mhz) => (mhz % 1000 === 0 ? String(mhz / 1000) : (mhz / 1000).toFixed(2));

/** A mode in words: `1920x1080 at 60 Hz`. */
export const modeWords = (mode) => `${mode.width}x${mode.height} at ${rateOf(mode.refresh_mhz)} Hz`;

/** Who has the screens, in words. */
export function frontWords(front) {
  if (front?.holder === 'program') {
    if (typeof front.process !== 'number') return 'A program that has not drawn yet';
    return front.name ? `${front.name} (process ${front.process})` : `Process ${front.process}`;
  }
  return front?.holder === 'last-picture' ? 'Nothing: the last picture of a program that ended' : 'The kernel’s log';
}

const yesNo = (yes) => (yes === true ? 'Yes' : 'No');

/** The monitor of a screen, as `{text, small}`. */
function monitorOf(screen) {
  const text = screen.monitor ?? (screen.maker ? `A monitor of ${screen.maker}` : 'A monitor that does not say what it is');
  const size = screen.width_mm && screen.height_mm ? `${screen.width_mm} by ${screen.height_mm} mm` : '';
  return { text, small: [screen.monitor && screen.maker, size].filter(Boolean).join(' · ') };
}

/** A screen as a row: `{name, monitor, small, muted, shows, tone, modes, depth, hdr, variable}`. */
function rowOf(screen) {
  const name = `Screen ${screen.number}`;
  if (screen.plugged !== true) {
    return { name, monitor: 'Nothing plugged in', small: '', muted: true, shows: DASH, modes: DASH, depth: DASH, hdr: DASH, variable: DASH };
  }
  const modes = Array.isArray(screen.modes) ? screen.modes : [];
  const failed = screen.failed === true;
  return {
    name,
    ...(({ text, small }) => ({ monitor: text, small }))(monitorOf(screen)),
    muted: false,
    shows: failed ? 'Stopped' : (screen.mode ? modeWords(screen.mode) : 'Nothing'),
    tone: failed ? 'bad' : null,
    modes: modes.map((mode) => `${modeWords(mode)}${mode.preferred ? ' (preferred)' : ''}`).join(', ') || DASH,
    depth: typeof screen.depth === 'number' ? `${screen.depth} bits a colour` : 'Not said',
    hdr: yesNo(screen.hdr),
    variable: yesNo(screen.variable_refresh),
  };
}

/** What a card is, after its name. */
function cardWords(card) {
  return [card.model, card.sets_modes === false && 'in the mode the firmware set', card.shared_pages === true && 'shows programs’ own pages'].filter(Boolean).join(', ');
}

/**
 * The screens of `answer` (`hardware screens`): `{front, cards: [{title,
 * rows}]}`, a card of the machine for each table.
 */
export function screensOf(answer) {
  const cards = Array.isArray(answer?.cards) ? answer.cards : [];
  return {
    front: frontWords(answer?.front),
    cards: cards.map((card) => ({
      title: [card.name, cardWords(card)].filter(Boolean).join(' · '),
      rows: (Array.isArray(card.screens) ? card.screens : []).map(rowOf),
    })),
  };
}

export const NO_SCREENS = 'No graphics card: the system draws on no screen.';
