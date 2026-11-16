// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What a test alert did, from the answer of `alerts.test`: the command's JSON,
// `{format, deliveries: [{channel, ok, error, error_id}]}`, comes back as a
// line of its output.

import { joinWords } from './channels-model.js';

/** What a test says when there is no channel to send it to (alerts(8)'s words). */
export const NO_CHANNEL_WORDS = 'No channel is set up: alerts go to this server\'s screens only.';

/** The deliveries in the answer's output, or null when no line is that. */
export function deliveriesOf(answer) {
  for (const line of Array.isArray(answer?.output) ? answer.output : []) {
    try {
      const found = JSON.parse(line);
      if (Array.isArray(found?.deliveries)) return found.deliveries;
    } catch {
      // A line that is not JSON is not the deliveries.
    }
  }
  return null;
}

/** A row for each delivery: `{name, ok, id, words}`, `words` being "email: sent" or "ntfy (ntfy.sh): failed: ntfy answered 403 Forbidden: forbidden". */
export function testRows(deliveries) {
  return deliveries.map((one) => ({
    name: one.channel,
    ok: one.ok === true,
    id: typeof one.error_id === 'string' ? one.error_id : null,
    words: one.ok === true ? `${one.channel}: sent` : `${one.channel}: failed${typeof one.error === 'string' && one.error !== '' ? `: ${one.error}` : ''}`,
  }));
}

/** The last test in a line, at `clock` ("7:20 AM"): "Last test reached email and ntfy (ntfy.sh) at 7:20 AM." and "It failed at …" for each channel that refused it. */
export function lastTestWords(rows, clock) {
  if (rows.length === 0) return NO_CHANNEL_WORDS;
  const reached = rows.filter((row) => row.ok).map((row) => row.name);
  return [
    reached.length > 0 && `Last test reached ${joinWords(reached)} at ${clock}.`,
    ...rows.filter((row) => !row.ok).map((row) => `It failed at ${row.name}.`),
  ].filter(Boolean).join(' ');
}

/** What the card of Security says of a test, `{words, bad}`, from the answer of `alerts.test` and the `clock` it was sent at. */
export function testSaid(answer, clock) {
  const deliveries = deliveriesOf(answer);
  if (deliveries === null) return { words: 'The test alert was sent.', bad: false };
  const rows = testRows(deliveries);
  return { words: lastTestWords(rows, clock), bad: rows.some((row) => !row.ok) };
}
