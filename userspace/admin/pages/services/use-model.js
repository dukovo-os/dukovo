// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Use card of the page of one service (design 4.4), as plain values: what
// its resource domain has used and been held to, from the `usage` of
// `service NAME status --json`, and the charts of its processor and memory
// over a range, from `service.usage`'s answer, which is `metrics show
// service.cpu{NAME} service.memory{NAME}` as JSON.

import { formatBytes, modelOf, rangeOf } from '../../lib/chart-model.js';
import { readyWords } from './detail-model.js';

/** `count` in the way a number is read: 12,345. */
const written = (count) => count.toLocaleString('en-US');

/** What the card says when the service has no numbers of its own yet. */
export const NO_USE = 'Spark has no numbers for it yet: it has not run since the machine started.';

/** What the card says when the machine has kept no history of the service. */
export const NO_HISTORY = 'No history yet: the machine keeps it from its first 10 minutes on.';

/** What the numbers are counted from. */
export const USE_FOOT = 'Counted since its first start after the machine started.';

/**
 * The rows of what the service has used: memory now and its peak, what its
 * memory limit did, threads, bytes read and written, and the time its
 * requests waited and were held back. A figure that is none says so, and
 * what the limit did is left out while it has done nothing. Empty for a
 * service that has no `usage`.
 */
export function useRows(usage) {
  if (!usage || typeof usage !== 'object') return [];
  const rows = [];
  if (Number.isFinite(usage.memory_bytes)) {
    const peak = Number.isFinite(usage.memory_peak_bytes) ? `, at most ${formatBytes(usage.memory_peak_bytes)} so far` : '';
    rows.push({ label: 'Memory now', value: `${formatBytes(usage.memory_bytes)}${peak}` });
  }
  if (usage.memory_refused > 0) {
    rows.push({ label: 'Memory limit', value: `It refused ${written(usage.memory_refused)} ${usage.memory_refused === 1 ? 'request' : 'requests'} for more memory` });
  }
  if (usage.stopped_at_limit === true) rows.push({ label: 'Last end', value: 'Stopped at its memory limit' });
  if (Number.isFinite(usage.threads)) rows.push({ label: 'Threads', value: written(usage.threads) });
  if (Number.isFinite(usage.read_bytes)) rows.push({ label: 'Read from disks', value: formatBytes(usage.read_bytes) });
  if (Number.isFinite(usage.written_bytes)) rows.push({ label: 'Written to disks', value: formatBytes(usage.written_bytes) });
  if (Number.isFinite(usage.waited_ms)) rows.push({ label: 'Waited for a disk', value: readyWords(usage.waited_ms) });
  if (Number.isFinite(usage.held_ms)) rows.push({ label: 'Held back by its rates', value: readyWords(usage.held_ms) });
  return rows;
}

/** The memory limit of the service's file in bytes, or null when it has none. */
export const memoryLimitOf = (service) => (service?.holds?.memory_bytes > 0 ? service.holds.memory_bytes : null);

/**
 * The charts of the card, `[{chart, total}]`: the processor in percent of one
 * processor, which a service of several threads may pass 100 of, and memory,
 * drawn up to its limit when the file sets one.
 */
export function chartsOf(service) {
  const total = memoryLimitOf(service);
  return [
    {
      chart: { id: 'svc-cpu', name: 'CPU', short: 'CPU', now: 'CPU now', kind: 'percent', scale: 'peak', lines: [{ series: 'service.cpu', label: 'CPU' }] },
      total: null,
    },
    {
      chart: { id: 'svc-memory', name: 'Memory', short: 'Memory', now: 'Memory', kind: 'bytes', scale: total === null ? 'peak' : 'total', lines: [{ series: 'service.memory', label: 'Memory' }] },
      total,
    },
  ];
}

/** The models of the charts for `answer`, over the range `id`, ending at `now` in Unix seconds. */
export function modelsOf(service, answer, id, now) {
  return chartsOf(service).map(({ chart, total }) => modelOf(chart, answer, { range: rangeOf(id), now, total }));
}

/** Whether the machine has kept nothing of the service: no point on any chart. */
export const noHistory = (models) => models.every((model) => model.empty);
