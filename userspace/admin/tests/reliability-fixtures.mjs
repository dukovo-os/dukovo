// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What admind answers for the Reliability page, as fixtures: the starts and
// the failed start (`changes --list --json` and `--failed --json`), one
// start's times (`startup --start N --compare --json`), the log's error
// records, the journal's rows, the crash dumps (`dumps list --json`) and the
// alerts' history (`alerts history --json`). The time is a Saturday morning
// in November, 2026, and the starts tell of an update that failed its check
// at 3:03 UTC, was taken back at 3:06 and was searched by changes bisect.

const at = (text) => Date.parse(text);

/** Now, in milliseconds and in seconds. */
export const NOW_MS = at('2026-11-14T10:12:00Z');
export const NOW = NOW_MS / 1000;

/** A start, with what a start leaves out null. */
const start = (number, when, release, slot, result, more = {}) => ({
  start: number, at_ms: at(when), release, slot, result, check: null, ready_ms: null, changes: [], ...more,
});

/** `reliability.starts`: newest first. */
export const starts = [
  start(44, '2026-11-14T04:10:00Z', '1.0.0-alpha.1', 'a', 'good', { ready_ms: 40500 }),
  start(43, '2026-11-14T04:05:00Z', '1.0.0-alpha.2', 'b', 'bisect-try'),
  start(42, '2026-11-14T04:00:00Z', '1.0.0-alpha.2', 'b', 'bisect-try'),
  start(41, '2026-11-14T03:06:00Z', '1.0.0-alpha.1', 'a', 'fell-back', { ready_ms: 41000 }),
  start(40, '2026-11-14T03:03:00Z', '1.0.0-alpha.2', 'b', 'failed', {
    check: 'BOOT-104',
    changes: [{ what: 'Dukovo', version: '1.0.0-alpha.2', at_ms: at('2026-11-14T03:02:00Z') }],
  }),
  start(39, '2026-11-11T02:01:00Z', '1.0.0-alpha.1', 'a', 'good', { ready_ms: 39000 }),
  start(38, '2026-11-02T22:40:00Z', '0.15.0', 'b', 'good', { ready_ms: 44000 }),
  start(37, '2026-10-31T23:30:00Z', '0.15.0', 'b', 'good', { ready_ms: 43000 }),
  start(36, '2026-10-20T16:12:00Z', '0.15.0', 'b', 'good', { ready_ms: 43000 }),
];

/** `reliability.failed`, while no search runs, and while one does. */
export const failed = {
  failed: starts[4],
  since_good: [
    { what: 'Dukovo', version: '1.0.0-alpha.2', at_ms: at('2026-11-14T03:02:00Z') },
    { what: 'Radix', version: '0.8.1', at_ms: at('2026-11-14T03:02:00Z') },
    { what: 'org.blog.blog', version: '2.1', at_ms: at('2026-11-13T21:10:00Z') },
  ],
  bisect: { running: false, try: null, of: null },
};
export const failedWhileSearching = { ...failed, bisect: { running: true, try: 1, of: 2 } };

/** One log record of an error, as `log -o json` prints it. */
const record = (time, name, text, seq, from = 'service') => ({
  time, boot: 'b1', seq, level: 'error', from, name, process: 411, user: null, audit: false, text,
});

/** The log's errors of the last days: `logs.read-all`. */
export const records = [
  record('2026-11-13T01:59:58.120000Z', 'backup-sync', 'The NAS at 192.168.1.20 did not answer.', 1),
  record('2026-11-13T02:00:03.000000Z', 'backup-sync', 'The NAS at 192.168.1.20 did not answer.', 2),
  record('2026-11-13T02:00:09.500000Z', 'nginx', 'connect() failed (111: Connection refused)', 3),
  record('2026-11-14T03:05:00.000000Z', 'nginx', `php-fpm answered 502 ${'x'.repeat(200)}`, 4),
  record('2026-11-14T03:05:01.000000Z', 'kernel', 'EXT4-fs error on vda1', 5, 'kernel'),
  record('2026-11-14T03:05:02.000000Z', 'nginx', 'php-fpm answered 502', 6),
];

/** A row of the journal, as `GET /api/1/changes` gives it. */
const change = (id, when, who, how, more = {}) => ({
  id, at: at(when) / 1000, who, how, action: null, title: null, argv: null, status: null, task: null, note: null, files: [], seen: null, ...more,
});

const file = (path) => ({ path, before: { state: 'absent' }, after: { state: 'text', sha256: 'ab', size: 2 } });

/** The journal's rows, newest first. */
export const changes = [
  change(96, '2026-11-14T08:30:00Z', 'alice', 'console', {
    action: 'settings.set', title: 'Change a setting', argv: ['settings', 'set', 'motd', 'it\'s a test'], status: 0, files: [file('/system/settings/motd')],
  }),
  change(95, '2026-11-14T03:40:00Z', 'ci', 'token ci', {
    action: 'firewall.add', title: 'Add a firewall rule', argv: ['firewall', 'add', 'allow', 'tcp', '8080'], status: 0,
  }),
  change(94, '2026-11-14T03:06:00Z', 'Spark', 'undone', { files: [file('/system/settings/network')] }),
  change(93, '2026-11-13T23:50:00Z', 'alice', 'console', {
    action: 'changes.put-back', title: 'Put a change back', argv: ['admin', 'put-back', '91'], status: 0,
  }),
  change(92, '2026-11-13T22:15:00Z', 'bob', 'console', {
    action: 'service.restart', title: 'Restart a service', argv: ['service', 'nginx', 'restart'], status: 1, note: 'the command ended with status 1',
  }),
  change(91, '2026-11-13T21:00:00Z', 'outside the console', 'by hand', { files: [file('/system/settings/blog.conf')] }),
  change(90, '2026-11-13T20:00:00Z', 'alice', 'console', {
    action: 'service.restart', title: 'Restart a service', argv: ['service', 'diskhealth', 'restart'], status: 0,
  }),
  change(89, '2026-11-13T19:00:00Z', 'alice', 'console', {
    action: 'console.export', title: 'Copy settings to a folder', argv: ['admin', 'export', '/users/alice/duke-nas-settings'], status: 0,
  }),
  change(88, '2026-11-13T18:00:00Z', 'alice', 'console', {
    action: 'gone.action', title: 'Do what this server cannot', argv: ['gone', 'now'], status: 0,
  }),
];

/** Who runs each action, as `GET /api/1/actions` says (only what the journal's rows use). */
export const runs = new Map([
  ['settings.set', 'as-administrator'],
  ['firewall.add', 'as-administrator'],
  ['service.restart', 'as-administrator'],
  ['console.export', 'as-the-person'],
  ['changes.put-back', 'as-administrator'],
]);

/** `reliability.dumps`: newest first, and one with no time. */
export const dumps = [
  { name: 'dump-20261114-0305.txt', at_ms: at('2026-11-14T03:05:00Z'), size: 48_211, first_line: 'panic: kernel BUG at mm/slab.c:411' },
  { name: 'dump-old.txt', at_ms: null, size: 2048, first_line: 'not a dump: it has no header' },
];

/** One line of `alerts history --json`. */
const alert = (when, state, severity, source, title) => ({
  at: at(when) / 1000, source, state, alert: { key: `${source}:x`, severity, title },
});

/** The alerts' history: older lines first and one after another, 22 of them. */
export const alerts = Array.from({ length: 22 }, (_, number) => alert(
  new Date(at('2026-11-13T00:00:00Z') + number * 3_600_000).toISOString(),
  number % 2 === 0 ? 'raised' : 'cleared',
  number % 4 === 0 ? 'critical' : 'warning',
  'diskhealth',
  `Disk ${number}`,
));

/** `reliability.start`: start 40's times beside start 39's (`startup --start 40 --compare --json`). */
export const startAnswer = {
  start: 40,
  outcome: 'failed',
  settled_ms: 9200,
  sign_in_ms: null,
  steps: [
    { name: 'firmware', ms: 1800 },
    { name: 'boot loader', ms: 900 },
    { name: 'kernel', ms: 1200 },
    { name: 'services', ms: 7100 },
    { name: 'sign-in', ms: null },
  ],
  chain: [
    { name: 'logd', started_ms: 1300, ready_ms: 1400, after: [] },
    { name: 'netd', started_ms: 1500, ready_ms: 2500, after: ['logd'] },
    { name: 'web', started_ms: 2600, ready_ms: 9200, after: ['netd'] },
  ],
  slowest: [
    { name: 'web', took_ms: 6600, started_ms: 2600 },
    { name: 'netd', took_ms: 1000, started_ms: 1500 },
  ],
  compared_with: {
    start: 39,
    settled_ms: 5000,
    steps: [
      { name: 'firmware', ms: 1800 },
      { name: 'boot loader', ms: 850 },
      { name: 'kernel', ms: 1200 },
      { name: 'services', ms: 2300 },
      { name: 'sign-in', ms: 12000 },
    ],
  },
};
