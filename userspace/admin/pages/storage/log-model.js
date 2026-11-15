// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the log's card says (design 4.6), as plain values: how much room the
// system's log takes, how long it keeps records, and the form of its limits
// with what each field takes. It is loaded when the card is drawn.

import { bytesWords, counted } from '../../lib/size-words.js';

/** How long a log keeps records, in words: "90 days", "a year", or null for no limit. */
export function keepWords(days) {
  if (!(days > 0)) return null;
  if (days === 365) return 'a year';
  return counted(days, 'day');
}

/**
 * The sentences about the log, from `GET /api/1/storage/log`'s answer: `{use,
 * keeps, wrong}`. "The system's log takes 1.2 GB in 92 files, 80 MB of it
 * the audit log." and "It keeps 90 days of records, and at most 4 GB or 10
 * percent of its disk, whichever is less; the audit log keeps a year."
 */
export function logWords(log) {
  const usage = log?.usage ?? {};
  const limits = log?.limits ?? {};
  const use = `The system's log takes ${bytesWords(usage.bytes)} in ${counted(usage.files ?? 0, 'file')}, ${bytesWords(usage.audit_bytes)} of it the audit log.`;
  const days = keepWords(limits.keep_days);
  const size = limits.max_bytes > 0 ? bytesWords(limits.max_bytes) : null;
  const percent = limits.max_percent > 0 ? `${limits.max_percent} percent of its disk` : null;
  const room = [size, percent].filter(Boolean);
  const keeps = [days ? `It keeps ${days} of records` : 'It keeps records for as long as there is room'];
  if (room.length === 2) keeps.push(`, and at most ${room[0]} or ${room[1]}, whichever is less`);
  else if (room.length === 1) keeps.push(`, and at most ${room[0]}`);
  const audit = keepWords(limits.audit_keep_days);
  keeps.push(audit ? `; the audit log keeps ${audit}.` : '; the audit log keeps its records for good.');
  return { use, keeps: keeps.join(''), wrong: Array.isArray(log?.wrong) ? log.wrong.map(String) : [] };
}

/** A size in bytes as the form shows it: "4G" for a whole number of gibibytes, else the bytes. */
export function sizeText(bytes) {
  if (!(bytes > 0)) return '';
  for (const [letter, size] of [['T', 1024 ** 4], ['G', 1024 ** 3], ['M', 1024 ** 2], ['K', 1024]]) {
    if (bytes % size === 0) return `${bytes / size}${letter}`;
  }
  return String(bytes);
}

/** `text` as bytes the way logtrim reads a size (a number, then K, M, G, T or P, with iB or B after it), or null. */
export function parseSize(text) {
  const found = /^(\d+)(?:([KMGTP])(IB|B)?)?$/i.exec(String(text).trim());
  if (!found) return null;
  const power = found[2] ? 'KMGTP'.indexOf(found[2].toUpperCase()) + 1 : 0;
  const base = String(found[3] ?? '').toUpperCase() === 'B' ? 1000 : 1024;
  const bytes = Number(found[1]) * base ** power;
  return Number.isSafeInteger(bytes) ? bytes : null;
}

/** The fields of the log's form, with what each takes and says. */
export const LOG_FIELDS = Object.freeze([
  { name: 'keep_days', label: 'Keep records for', unit: 'days', hint: 'A whole number of days from 1 to 3650, or empty for no limit.', min: 1, max: 3650 },
  { name: 'max_size', label: 'Take at most', unit: null, hint: 'A size such as 4G or 500M, or empty for no limit.' },
  { name: 'max_percent', label: 'Take at most', unit: 'percent of its disk', hint: 'A whole number from 1 to 100, or empty for no limit.', min: 1, max: 100 },
  { name: 'audit_keep_days', label: 'Keep the audit log for', unit: 'days', hint: 'A whole number of days from 30 to 3650, or empty for no limit.', min: 30, max: 3650 },
]);

/** What the form starts with, as text, from the log's limits. */
export function formOf(limits) {
  return {
    keep_days: limits?.keep_days > 0 ? String(limits.keep_days) : '',
    max_size: sizeText(limits?.max_bytes),
    max_percent: limits?.max_percent > 0 ? String(limits.max_percent) : '',
    audit_keep_days: limits?.audit_keep_days > 0 ? String(limits.audit_keep_days) : '',
  };
}

/** Whether `text` means no limit: nothing, 0, or those words. */
const noLimit = (text) => /^(|0|no limit)$/i.test(text.trim());

/**
 * The form's text as the arguments of `storage.log-set`, with only what
 * differs from `limits`: `{args}`, or `{errors}` of field to words, or
 * `{nothing: true}` when nothing differs. A limit lifted is 0, as the
 * command takes it.
 */
export function logArgs(form, limits) {
  const errors = {};
  const args = {};
  const current = formOf(limits);
  for (const field of LOG_FIELDS) {
    const text = String(form[field.name] ?? '').trim();
    if (noLimit(text)) {
      if (current[field.name] !== '') args[field.name] = field.name === 'max_size' ? '0' : 0;
      continue;
    }
    if (field.name === 'max_size') {
      const bytes = parseSize(text);
      if (bytes === null || bytes <= 0) errors[field.name] = 'Use a size such as 4G or 500M, or leave it empty for no limit.';
      else if (bytes !== limits?.max_bytes) args[field.name] = text;
      continue;
    }
    const number = /^\d+$/.test(text) ? Number(text) : null;
    if (number === null || number < field.min || number > field.max) errors[field.name] = field.hint;
    else if (number !== (limits?.[field.name] ?? 0)) args[field.name] = number;
  }
  if (Object.keys(errors).length > 0) return { errors };
  if (Object.keys(args).length === 0) return { nothing: true };
  return { args };
}
