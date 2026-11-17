// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The certificates that could not be got and that certd asks for again, from
// `cert wanted --json`: each as a record, and the words of its row in the
// list. A record is not a certificate: it has no end, no use and no key.

import { splitId } from './history-model.js';

/** What the list says when the records could not be read. */
export const WANTED_LOST_WORDS = 'The certificates that certd tries to get again could not be read.';

/**
 * The records of `certificate.wanted`'s answer: `{name, more, nextMs, tries,
 * why, id, givenUp}`. `more` counts the names after the first; `why` is the
 * last reason without its error ID, which is `id`. Any other value is left out.
 */
export function wantedOf(answer) {
  const found = Array.isArray(answer?.wanted) ? answer.wanted : [];
  return found.filter((one) => typeof one?.name === 'string' && one.name !== '').map((one) => {
    const { words, id } = splitId(one.why);
    return {
      name: one.name,
      more: Array.isArray(one.names) ? Math.max(0, one.names.length - 1) : 0,
      nextMs: Number.isFinite(one.next_ms) ? one.next_ms : null,
      tries: Number.isInteger(one.tries) && one.tries >= 0 ? one.tries : 0,
      why: words.trim().replace(/[.\s]+$/, ''),
      id,
      givenUp: one.given_up === true,
    };
  });
}

/** The words of a row: "Trying again: next at 4:05 PM, 3 tries, last: why", or for a record given up, "Given up: why. Get it again to start afresh." */
export function wantedWords(record, time) {
  if (record.givenUp) return `Given up${record.why && `: ${record.why}`}. Get it again to start afresh.`;
  const parts = [
    record.nextMs !== null && `next at ${time.when(record.nextMs / 1000)}`,
    record.tries === 1 ? '1 try' : `${record.tries} tries`,
    record.why && `last: ${record.why}`,
  ];
  return `Trying again: ${parts.filter(Boolean).join(', ')}`;
}
