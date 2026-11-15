// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Limits and Sockets cards of the page of one service (design 4.4), as
// plain values: what the service's file lets its resource domain use, and
// the sockets Spark holds for it, from what `service NAME status --json`
// says of `holds`, `stop_signal`, `stop_timeout_s` and `state`.

import { formatBytes } from '../../lib/chart-model.js';
import { isUp } from './model.js';

/** `count` in the way a number is read: 4,096. */
const written = (count) => count.toLocaleString('en-US');

/** The strings of `list` that are not empty, or none when it is not a list. */
const namesOf = (list) => (Array.isArray(list) ? list.filter((one) => typeof one === 'string' && one !== '') : []);

/** The place of the file of the service `name`, which `service try` changes. */
export const fileOf = (name) => `/system/settings/spark/services/${name}.toml`;

/** The command that changes the limits, as service(8) has it: it edits the file and undoes the change unless it is kept. */
export const tryCommand = (name) => `elevate service try ${name}`;

/** A rate a second that a file may leave without a limit: "No limit" or "50 MB a second". */
const rateWords = (value, format) => (Number.isFinite(value) ? `${format(value)} a second` : 'No limit');

/**
 * The rows of the Limits card: `[{label, value}]`. A limit the file does not
 * set says so, and the weights and the threads are the numbers the file
 * gives or its defaults. The rows are empty for a service that has no
 * `holds`.
 */
export function limitsRows(service) {
  const holds = service?.holds;
  if (!holds || typeof holds !== 'object') return [];
  const rows = [
    { label: 'Memory', value: Number.isFinite(holds.memory_bytes) ? formatBytes(holds.memory_bytes) : 'No limit' },
    { label: 'CPU weight', value: written(holds.cpu_weight) },
    { label: 'Threads', value: `At most ${written(holds.threads)}` },
    { label: 'Open files', value: Number.isFinite(holds.open_files) ? `${written(holds.open_files)} for each process` : 'The usual 1,024 for each process' },
    { label: 'Disk weight', value: written(holds.io_weight) },
    { label: 'Disk reads', value: rateWords(holds.io_read, formatBytes) },
    { label: 'Disk writes', value: rateWords(holds.io_write, formatBytes) },
    { label: 'Read requests', value: rateWords(holds.io_read_ops, (count) => `${written(count)} requests`) },
    { label: 'Write requests', value: rateWords(holds.io_write_ops, (count) => `${written(count)} requests`) },
  ];
  if (typeof service.stop_signal === 'string' && service.stop_signal !== '') rows.push({ label: 'Stop signal', value: service.stop_signal });
  if (Number.isFinite(service.stop_timeout_s)) rows.push({ label: 'Stop timeout', value: `${service.stop_timeout_s} s, then Spark ends it` });
  rows.push({ label: 'Its file', value: fileOf(service.name) });
  return rows;
}

/** Where the service stands with its sockets, in words. */
function standing(service) {
  if (service.state === 'listening') return 'Waiting for a connection. The first one starts the service.';
  return isUp(service) ? 'Running, and takes the connections' : 'Not running';
}

/**
 * The rows of the Sockets card: each socket Spark holds for the service, the
 * groups its sockets put it in, and whether it listens now; null for a
 * service with none of them.
 */
export function socketsRows(service) {
  const sockets = namesOf(service?.holds?.sockets);
  const groups = namesOf(service?.holds?.groups);
  if (sockets.length === 0 && groups.length === 0) return null;
  return [
    sockets.length > 0 && { label: 'Sockets', items: sockets },
    sockets.length > 0 && { label: 'Now', value: standing(service) },
    groups.length > 0 && { label: 'Also in the groups', items: groups },
  ].filter(Boolean);
}
