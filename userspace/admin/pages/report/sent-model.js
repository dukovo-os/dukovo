// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The table of the reports from this server as plain values: the reports
// that wait and those sent, where each stands, and the words of the question
// before one is deleted. No DOM.

import { counted } from '../../lib/size-words.js';
import { listOf } from './model.js';

const text = (value) => (typeof value === 'string' && value.trim() !== '' ? value.trim() : null);

/** The kind a status gets: a report fixed is good, one that will not be is plain, the others wait. */
const statusKind = (words) => (/^Fixed/.test(words) ? 'ok' : /^Will not/.test(words) ? 'warn' : 'off');

/** Words for a time given in milliseconds since 1970, or "" when it is none. */
const whenWords = (time, ms) => (Number.isFinite(ms) ? time.when(ms / 1000) : '');

/**
 * The rows of the table: the reports that wait first, then those sent, the
 * newest first. `status` is what `reports.status` answered, a Map of the
 * number to its row, or null when it was not asked; `asked` is `{why}` when
 * asking failed. Each row is `{number, sent, title, status, note, kind,
 * canDelete, queued}`.
 */
export function tableRows(list, { status = null, asked = null, time }) {
  const reports = listOf(list, 'reports').filter((report) => Number.isFinite(report?.number) || report?.state === 'queued');
  const order = (report) => (report.state === 'queued' ? Number.MAX_SAFE_INTEGER : (report.sent_ms ?? report.queued_ms ?? 0));
  return [...reports].sort((a, b) => order(b) - order(a)).map((report) => {
    const row = {
      number: report.number ?? null,
      title: text(report.title) ?? 'A report',
      sent: report.state === 'sent' ? whenWords(time, report.sent_ms) : report.state === 'queued' ? 'Waiting' : 'Not sent',
      canDelete: report.state === 'sent',
      queued: report.state === 'queued',
      note: null,
    };
    if (report.state === 'queued') {
      const tried = report.tries > 0 ? ` · tried ${counted(report.tries, 'time')}` : '';
      const next = Number.isFinite(report.next_ms) ? `, next at ${time.when(report.next_ms / 1000)}` : '';
      return { ...row, kind: 'off', status: 'Queued', note: `Sends when reports.dukovo.org can be reached${tried}${next}${text(report.why) ? `, last: ${report.why}` : ''}` };
    }
    if (report.state === 'failed') {
      return { ...row, kind: 'bad', status: 'Could not be sent', note: text(report.why) ?? 'It was tried for 30 days.' };
    }
    if (asked) return { ...row, kind: 'warn', status: `Could not ask: ${asked.why}`, note: null };
    const got = status?.get(report.number);
    if (got?.unreachable) return { ...row, kind: 'warn', status: 'Could not ask: reports.dukovo.org did not answer', note: null };
    if (got) {
      const words = text(got.status) ?? 'Received';
      return { ...row, kind: statusKind(words), status: words, note: null };
    }
    return { ...row, kind: 'off', status: 'Not asked yet', note: 'Check the status to ask the server.' };
  });
}

/** The status answers of `reports.status` as a Map of number to row. */
export const statusMap = (answer) => new Map(listOf(answer, 'reports').filter((one) => Number.isFinite(one?.number)).map((one) => [one.number, one]));

/** The words of an error, to say why the server could not be asked. */
export const whyWords = (error) => [error?.title, error?.detail].filter(Boolean).join(': ') || 'no answer';

/** What the delete question says. */
export const DELETE_WORDS = 'Deletes it on the reports server and here.';
