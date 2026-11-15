// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Services pages read, as admind answers it, for the tests of
// their logic: Spark's `show` document with a service in each state, the
// objects of `service NAME status --json` and `service NAME explain --json`,
// and the timers `service list --timers --json` prints.
// The time is the Overview fixtures': Saturday Nov 14 2026, 10:12 UTC, which
// is 17,520,000 ms after the machine started.

import { NOW } from './overview-fixtures.mjs';

export { NOW };

/** Milliseconds since the machine started at the moment the fixtures call now. */
export const NOW_MS = 17_520_000;

/** A service as Spark's `show` gives it, `ago` seconds in its state. */
export const service = (name, description, state, ago, more = {}) => ({
  name, description, enabled: true, state, since_ms: NOW_MS - ago * 1000, failures: 0, restarts: 0, last_end: null, last_line: null, why: null, off_as_meant: false, ...more,
});

/** `GET /api/1/services`: a service in each state Spark names, and two trials. */
export const document = {
  show_version: 1,
  now_ms: NOW_MS,
  services: [
    service('admind', 'This console', 'running', 17_000),
    service('nginx', 'Web server', 'running', 3600, { pid: 312, started_ms: NOW_MS - 3_600_000, ready_ms: NOW_MS - 3_599_000 }),
    service('backup-sync', 'Copies backups to the NAS', 'failed', 10_800, {
      failures: 3, last_end: 'exited with code 1', last_line: 'the NAS at 192.168.1.20 did not answer',
    }),
    service('printer', 'Print server', 'disabled', 90_000, { enabled: false, off_as_meant: true, why: 'it is off until it is started' }),
    service('db', 'Database', 'pending', 5, { waiting_for: ['disk'] }),
    service('web2', 'Second web server', 'starting', 4, { pid: 90 }),
    service('mailer', 'Mail relay', 'waiting', 30, { retry_ms: NOW_MS + 12_000 }),
    service('logrotate', 'Rotates the logs', 'done', 600),
    service('tunnel', 'Tunnel to the office', 'stopping', 2),
    service('Notes_2', 'Notes for the office', 'stopped', 800, { enabled: false }),
  ],
  shutdown: null,
  trials: [
    { id: 'service.nginx', files: ['/system/settings/spark/services/nginx.toml'], services: ['nginx'], user: 0, left_ms: 42_000 },
    { id: 'network', files: ['/system/settings/network'], services: ['netd'], user: 0, left_ms: 290_000 },
  ],
  undone: [],
};

/** The services as Spark gives them with #390's fields: where they start, and whether a timer starts them. */
export const withStarts = {
  ...document,
  services: [
    service('web', 'Web server', 'listening', 900, { start: 'on-connection', timer: false }),
    service('cleanup', 'Cleans the cache', 'stopped', 900, { start: 'at-boot', timer: true }),
    service('syncer', 'Syncs files', 'running', 900, { start: 'at-boot', timer: false, enabled: false }),
  ],
  trials: [],
};

/** `GET /api/1/services/nginx`: `service nginx status --json`, with the clock of the document beside the service. */
export const status = {
  now_ms: NOW_MS,
  service: { ...document.services[1], restart: 'on-failure', give_up_after: 5, requires: ['netd'], wants: ['logd'], after: ['netd', 'logd'], started_by: 'boot', restarts: 2 },
  processes: [{ pid: 312, user: 'www', program: '/system/commands/nginx' }, { pid: 313, user: 'www', program: '/system/commands/nginx' }],
  listening: [{ port: 443, tcp: true }],
  trial: null,
};

/** `GET /api/1/services/backup-sync/explain`. */
export const explanation = {
  sentence: 'backup-sync is not running: its start failed 3 times in a row; it last said: the NAS at 192.168.1.20 did not answer.',
  chain: ['backup-sync'],
  root: 'backup-sync',
  fix: 'Check that the NAS answers, then run: service backup-sync start.',
};

/** What `service list --timers --json` prints (`service.timers`): a schedule of each kind, a run that failed and one never run. Times are in the zone of the timer. */
export const timers = [
  {
    service: 'backup-sync', schedule: '0 3 * * *', next_ms: Date.UTC(2026, 10, 15, 2, 0), last_ms: Date.UTC(2026, 10, 14, 2, 0), last_result: 'failed', zone: 'Europe/Prague',
  },
  {
    service: 'cleanup', schedule: 'every 6h, first 10m after boot', next_ms: Date.UTC(2026, 10, 14, 12, 0), last_ms: Date.UTC(2026, 10, 14, 6, 0), last_result: 'ok', zone: 'UTC',
  },
  {
    service: 'logrotate', schedule: null, next_ms: null, last_ms: null, last_result: null, zone: null,
  },
  {
    service: 'certs', schedule: '30 4 * * 1-5, daily', next_ms: Date.UTC(2026, 10, 14, 11, 0), last_ms: null, last_result: null, zone: 'Europe/Prague',
  },
];

/** The service of `status` as Spark gives it for `show NAME` with #390's fields: its limits, sockets, stop and what its domain has used. */
export const withLimits = {
  ...status.service,
  start: 'on-connection',
  timer: false,
  stop_signal: 'SIGTERM',
  stop_timeout_s: 5,
  holds: {
    account: 'www', reads: [], changes: [], memory_bytes: 268_435_456, cpu_weight: 100, io_weight: 200, io_read: 52_428_800, io_write: null, io_read_ops: null, io_write_ops: 500,
    threads: 4096, open_files: 65_536, sockets: ['port 80', 'port 443'], groups: ['web'],
  },
  usage: {
    memory_bytes: 148_000_000, memory_peak_bytes: 212_000_000, memory_refused: 3, stopped_at_limit: true, threads: 14, read_bytes: 9_800_000_000, written_bytes: 1_200_000, waited_ms: 850, held_ms: 12_400,
  },
};

/** `GET /api/1/services/nginx/usage/24h`: `metrics show` of the service's processor and memory. */
export const usageAnswer = [
  { series: 'service.cpu{nginx}', points: [[NOW - 1800, 3.5], [NOW - 900, 12], [NOW, 6.2]] },
  { series: 'service.memory{nginx}', points: [[NOW - 1800, 140_000_000], [NOW - 900, 212_000_000], [NOW, 148_000_000]] },
];
