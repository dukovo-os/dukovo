// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Overview's parts and cards read, as admind answers it, for the
// tests of their logic and for the fake server that the board checks use.
// The times are Unix seconds, and "now" is Saturday Nov 14 2026, 10:12 UTC.

/** Unix seconds of a UTC time. */
export const utc = (year, month, day, hour = 0, minute = 0) => Date.UTC(year, month - 1, day, hour, minute) / 1000;

/** The instant the fixtures are "now". */
export const NOW = utc(2026, 11, 14, 10, 12);

const service = (name, description, state, more = {}) => ({
  name, description, enabled: true, state, since_ms: 1000, failures: 0, restarts: 0, last_end: null, last_line: null, why: null, off_as_meant: false, ...more,
});

/** `GET /api/1/services`: Spark's document. The machine has been up 4 h 52 min. */
export const services = {
  show_version: 1,
  now_ms: 17_520_000,
  services: [
    service('postgres', 'PostgreSQL database', 'running'),
    service('nginx', 'Web server', 'running'),
    service('php-fpm', 'PHP pages for photos.example.org', 'running'),
    service('valkey', 'Valkey cache', 'running'),
    service('admind', 'This console', 'running'),
    service('backup-sync', 'Copies backups to the NAS', 'failed', {
      since_ms: 6_720_000, failures: 3, last_end: 'exited with code 1', last_line: 'the NAS at 192.168.1.20 did not answer',
    }),
    service('printer', 'Print server', 'stopped', { off_as_meant: true, why: 'it is off until it is started' }),
    service('nightly', 'Nightly jobs', 'disabled', { enabled: false }),
  ],
  shutdown: null,
  trials: [],
  undone: [],
};

const certificate = (name, daysLeft, more = {}) => ({
  name, kind: 'acme', names: [`${name}.example.org`], not_before: '2026-09-01T00:00:00Z', not_after: '2027-01-01T00:00:00Z', days_left: daysLeft,
  fingerprint_sha256: 'ab'.repeat(32), issuer: 'CN=R3', trusted: true, used_by: [], ...more,
});

/** `GET /api/1/certificates`: `cert list --json`. */
export const certificates = [
  certificate('photos', 60, { used_by: ['nginx'] }),
  certificate('blog', 9, { used_by: ['nginx', 'valkey'] }),
  certificate('home', 1, { kind: 'self-signed' }),
  certificate('old', -3, { kind: 'added', used_by: ['mail'] }),
  certificate('stolen', 40, { revoked: true }),
];

const task = (id, action, title, state, more = {}) => ({
  id, action, title, args: {}, who: 'chase', how: 'console', started: NOW - 3600, state, step: 0, steps: 0, words: '', percent: null, ended: NOW - 3000, status: null, note: null, changed: true, ...more,
});

/** `GET /api/1/tasks`. */
export const tasks = {
  tasks: [
    task(41, 'certificate.renew', 'Renew a certificate', 'failed', { args: { name: 'blog' }, status: 1, note: 'the authority did not answer', ended: NOW - 5400 }),
    task(40, 'service.restart', 'Restart a service', 'stopped', { args: { name: 'nginx' }, ended: NOW - 7200 }),
    task(39, 'service.start', 'Start a service', 'done', { args: { name: 'printer' } }),
    task(30, 'certificate.renew', 'Renew a certificate', 'failed', { args: { name: 'old' }, status: 2, ended: NOW - 3 * 86400 }),
    task(42, 'lab.long', 'Run the lab\'s long test', 'running', { ended: null }),
  ],
};

const file = (path, before = { state: 'text', sha256: 'aa' }, after = { state: 'text', sha256: 'bb' }) => ({ path, before, after });

/** `GET /api/1/changes?since=…&limit=…`: the journal. */
export const changes = {
  changes: [
    {
      id: 12, at: NOW - 9000, who: 'alice', how: 'by hand', action: null, title: null, argv: null, status: null, task: null, note: null,
      files: [file('/system/settings/nginx/sites/blog.conf')], seen: null,
    },
    {
      id: 11, at: NOW - 86400 - 7200, who: 'chase', how: 'console', action: 'service.restart', title: 'Restart a service', argv: ['service', 'nginx', 'restart'], status: 0, task: null, note: null,
      files: [], seen: null,
    },
    {
      id: 10, at: NOW - 2 * 86400, who: 'outside the console', how: 'by hand', action: null, title: null, argv: null, status: null, task: null, note: null,
      files: [file('/system/settings/ssh/sshd_config'), file('/system/settings/ssh/banner')], seen: null,
    },
    {
      id: 9, at: NOW - 3 * 86400, who: 'alice', how: 'by hand', action: null, title: null, argv: null, status: null, task: null, note: null,
      files: [file('/system/settings/a.conf')], seen: { by: 'chase', at: NOW - 86400 },
    },
    {
      id: 8, at: NOW - 9 * 86400, who: 'alice', how: 'by hand', action: null, title: null, argv: null, status: null, task: null, note: null,
      files: [file('/system/settings/old.conf')], seen: null,
    },
  ],
};

/** `GET /api/1/changes/12`: one change with its diffs and what puts it back. */
export const change12 = {
  ...changes.changes[0],
  files: [{
    ...file('/system/settings/nginx/sites/blog.conf'),
    diff: '--- /system/settings/nginx/sites/blog.conf (before)\n+++ /system/settings/nginx/sites/blog.conf (after)\n@@ -1,3 +1,3 @@\n server {\n-  listen 80;\n+  listen 8080;\n }\n',
    diff_truncated: false,
  }],
  put_back: { possible: true, command: 'elevate admin put-back 12' },
};

/** `GET /api/1/console/about`: what `admin about --json` prints. */
export const about = {
  host: 'duke-nas',
  addresses: ['192.168.1.20'],
  port: 9090,
  fingerprint: 'cd'.repeat(32),
  release: 'Dukovo Server 1.0.0-alpha.1',
  fallback: false,
  machine: { maker: 'Dell Inc.', model: 'OptiPlex 7040' },
  processor: 'Intel Core i5-6500',
  memory: 16_000_000_000,
  up: 17_520,
  zone: 'UTC',
  clock: { synchronized: true },
  ssh_host_key: 'SHA256:q3VnAbCdEfGhIjKlMnOpQrStUvWxYz0123456789f9Ke',
  last_good_start: NOW - 4 * 3600,
  sign_ins: [
    { at: NOW - 3000, terminal: 'console', from: '192.168.1.25' },
    { at: NOW - 86400 - 4 * 3600, terminal: 'pts/0', from: '10.8.0.2' },
  ],
};

/** `GET /api/1/console/notices`. */
export const notices = {
  notices: [
    {
      name: 'failed-start',
      lines: ['This server went back to Dukovo 1.0.0-alpha.1 at 3:06 AM.', 'The update to alpha.2 installed, but nginx failed a required check after the restart.'],
      at: NOW - 7 * 3600,
    },
  ],
};

/** The result `dukovo-check --last --json` keeps, which `GET /api/1/checks/last` gives. */
export const checks = {
  version: 1,
  checked: '2026-11-14T05:17:03Z',
  release: '1.0.0-alpha.1',
  results: [
    { area: 'network', check: 'firewall rules', result: 'passed', detail: '2 rules', id: null, fix: null },
    { area: 'ssh', check: 'passwords', result: 'warning', detail: 'passwords sign in over SSH (sshd_config: PasswordAuthentication yes)', id: 'SEC-110', fix: 'Add your SSH key, then set PasswordAuthentication no in /system/settings/ssh/sshd_config.' },
    { area: 'updates', check: 'waiting', result: 'warning', detail: 'Dukovo 1.0.0-alpha.2 is waiting', id: 'SEC-130', fix: 'Install the update from Updates.' },
    { area: 'services', check: 'failed', result: 'warning', detail: 'backup-sync gave up after its restarts', id: 'SEC-150', fix: 'Run service explain backup-sync.' },
    { area: 'health', check: 'hardware', result: 'skipped', detail: 'hardware check has not run', id: null, fix: null },
  ],
  counts: { passed: 1, warning: 3, failed: 0, skipped: 1 },
};

/** `GET /api/1/second-step`'s list, as `GET /api/1/account/second-step` gives it. */
export const secondStep = { app: null, passkeys: [], recovery: null, trusted: 0 };

/** `GET /api/1/metrics/now`. */
export const metricsNow = {
  cpu: 18.4,
  'memory.used': 7_400_000_000,
  'service.cpu{postgres}': 3.1, 'service.memory{postgres}': 1_900_000_000,
  'service.cpu{nginx}': 0.4, 'service.memory{nginx}': 210_000_000,
  'service.cpu{php-fpm}': 1.2, 'service.memory{php-fpm}': 184_000_000,
  'service.cpu{valkey}': 0.2, 'service.memory{valkey}': 96_000_000,
  'service.cpu{admind}': 0.1, 'service.memory{admind}': 22_000_000,
  'service.cpu{backup-sync}': 0, 'service.memory{backup-sync}': 0,
};

/**
 * `GET /api/1/metrics/usage/{range}`: the seven series over `span` seconds
 * ending at `NOW`, a point every `every` seconds, with a spike of the CPU
 * and the disk a little before the end.
 */
export function usage({ span, every }) {
  const count = Math.floor(span / every);
  const at = (index) => NOW - (count - index) * every;
  const series = (name, value) => ({ series: name, points: Array.from({ length: count + 1 }, (_, index) => [at(index), value(index, count)]) });
  const spike = (index, last, high, low) => (index === Math.floor(last * 0.4) ? high : low);
  return [
    series('cpu', (index, last) => spike(index, last, 92, 8 + (index % 7))),
    series('memory.used', (index) => 7_200_000_000 + (index % 5) * 40_000_000),
    series('memory.available', (index) => 8_800_000_000 - (index % 5) * 40_000_000),
    series('disk.read{vda}', (index, last) => spike(index, last, 2_100_000, 20_000)),
    series('disk.write{vda}', (index, last) => spike(index, last, 1_400_000, 12_000)),
    series('net.rx{eth0}', (index) => 200_000 + (index % 11) * 150_000),
    series('net.tx{eth0}', (index) => 60_000 + (index % 13) * 30_000),
  ];
}

/**
 * The `cx` a part gets (design 3.5), over `answers`, which maps a path, or
 * the start of one, to what `GET` gives: a value, or a function of the full
 * path. `calls` lists the paths read. A path with no answer fails as a
 * server that does not answer does.
 */
export function fakeCx(answers, session = { name: 'chase', admin: true }) {
  const calls = [];
  const kept = new Map();
  const get = async (path) => {
    calls.push(path);
    const key = Object.keys(answers).find((start) => path === start || path.startsWith(`${start}?`));
    if (key === undefined) throw Object.assign(new Error(`no answer for ${path}`), { status: 0 });
    const answer = answers[key];
    return typeof answer === 'function' ? answer(path) : answer;
  };
  const cache = (key, ms, make) => {
    if (!kept.has(key)) kept.set(key, Promise.resolve().then(make));
    return kept.get(key);
  };
  return { calls, session, time: null, api: { get }, cache };
}
