// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What a hold's reason may be, as plain values: the one line of at most 200
// characters that `update hold` and `bundle hold` keep with a hold, and the
// words of the note that says who reads it. Read with the dialog that asks
// for it (lib/hold-dialog.js).

/** The most characters a reason may have. */
export const WHY_MOST = 200;

/**
 * The reason a person typed as an action takes it: `{why, problem}`. `why` is
 * the text with the spaces around it taken off, or null when there is none
 * (a hold needs no reason); `problem` is what is wrong with it, in words for
 * the field, or null.
 */
export function whyOf(words) {
  const why = String(words ?? '').trim();
  if (why === '') return { why: null, problem: null };
  if (/[\x00-\x1f\x7f]/.test(why)) return { why: null, problem: 'The reason is one line: it cannot hold a line break or a tab.' };
  if ([...why].length > WHY_MOST) return { why: null, problem: `The reason is at most ${WHY_MOST} characters.` };
  return { why, problem: null };
}

/** The note under the field, who reads the reason and where, as its parts: the words, and the command that shows it beside the hold, `place`, to be set as code. */
export const whyNote = (place) => ['Everyone who can sign in sees the reason, here and with ', place, '.'];
