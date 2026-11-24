// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What admind answers the AI clients page, as the tests need it: the areas
// of `GET /api/1/clients/areas`, the requests that wait, the history, and
// the clients as authd lists them, in the forms that admind's own tests
// show. Not a test itself, and never installed.

import { makeTime } from '../lib/time.js';

/** 9:43 AM on Tuesday 24 November 2026, UTC. */
export const NOW = Date.UTC(2026, 10, 24, 9, 43, 0);
export const time = makeTime('UTC', () => NOW);

/** Now, in the seconds that admind and authd count in. */
export const AT = NOW / 1000;
const DAY = 86_400;

const area = (name, title, reads, readsAll, asks, never = 0, whoGetsIn = false) => ({
  area: name, title, reads, reads_all: readsAll, asks, never, who_gets_in: whoGetsIn,
});

/**
 * Eight of the console's areas, in its order, one of each kind: with every
 * level, with nothing only an administrator reads, with nothing to ask
 * for, with nothing to read, and two that decide who gets in.
 */
export const areas = () => ({
  areas: [
    area('services', 'Services', 6, 2, 8),
    area('logs', 'Logs', 2, 2, 0),
    area('storage', 'Storage', 5, 0, 4),
    area('certificates', 'Certificates', 3, 0, 3),
    area('updates', 'Updates', 3, 1, 5),
    area('power', 'Restart and shut down', 0, 0, 2),
    area('accounts', 'Accounts', 2, 2, 0, 9, true),
    area('console', 'The console\'s settings', 1, 3, 0, 6, true),
  ],
});

/** One client of a list, with what a test says over it. */
export const client = (more = {}) => ({
  owner: 'chase',
  name: 'workstation-assistant',
  way: 'ssh',
  scopes: ['*:read', 'services', 'certificates', 'updates'],
  until: AT + 90 * DAY,
  made: AT - 2 * DAY,
  used: AT - 130,
  from: '192.168.1.20',
  ended: false,
  ...more,
});

const NIGHT_WATCH = {
  name: 'night-watch', way: 'https', scopes: ['logs:read', 'services:read', 'storage:read'], until: AT + 30 * DAY, used: AT - 6 * 3600 - 41 * 60, from: '192.168.1.31',
};

/** `GET /api/1/account/clients` for chase: the two clients of his own. */
export const own = () => ({ clients: [client(), client(NIGHT_WATCH)] });

/** `GET /api/1/console/clients`: every account's, with one of alice's that has ended and was never used. */
export const every = () => ({
  clients: [
    ...own().clients,
    client({
      owner: 'alice', name: 'alice-laptop', scopes: ['services:read', 'logs:read'], until: AT - 4 * DAY, used: 0, from: '', ended: true,
    }),
  ],
});

/** One request that waits, with what a test says over it. */
export const request = (more = {}) => ({
  request: 12,
  owner: 'chase',
  client: 'workstation-assistant',
  action: 'service.restart',
  title: 'Restart a service: nginx',
  area: 'services',
  args: { name: 'nginx' },
  reason: 'nginx stopped answering after last night\'s certificate renewal. A restart loads the new certificate.',
  asked: AT - 130,
  lapses: AT - 130 + 3600,
  command: 'elevate service nginx restart',
  files: [],
  ...more,
});

/** `GET /api/1/clients/requests` with the requests given, or the one above. */
export const requests = (...waiting) => ({ requests: waiting.length > 0 ? waiting : [request()] });

/** `GET /api/1/clients/history`: one entry of each kind and state, the newest first. */
export const history = () => ({
  entries: [
    {
      at: AT - 130, owner: 'chase', client: 'workstation-assistant', kind: 'ask', request: 12, action: 'service.restart', title: 'Restart a service: nginx', state: 'waiting',
    },
    {
      at: AT - 190, owner: 'chase', client: 'workstation-assistant', kind: 'read', since: AT - 260, count: 6, areas: ['services', 'logs', 'certificates'],
    },
    {
      at: AT - 1860, owner: 'chase', client: 'workstation-assistant', kind: 'ask', request: 11, action: 'app.update', title: 'Update an app: valkey', state: 'task', by: 'chase', task: 1841, ended: AT - 1800,
    },
    {
      at: AT - 3000, owner: 'alice', client: 'alice-laptop', kind: 'read', since: AT - 3000, count: 1, areas: ['logs'],
    },
    {
      at: AT - 3600, owner: 'chase', client: 'workstation-assistant', kind: 'ask', request: 10, action: 'certificate.renew', title: 'Renew a certificate: this-machine', state: 'done', by: 'chase', ended: AT - 3540,
    },
    {
      at: AT - 7200, owner: 'chase', client: 'workstation-assistant', kind: 'ask', request: 9, action: 'service.restart', title: 'Restart a service: valkey', state: 'failed', by: 'alice', ended: AT - 7100,
    },
    {
      at: AT - DAY, owner: 'chase', client: 'workstation-assistant', kind: 'ask', request: 8, action: 'storage.scrub', title: 'Check a volume: data', state: 'refused', by: 'chase',
    },
    {
      at: AT - 2 * DAY, owner: 'chase', client: 'night-watch', kind: 'ask', request: 7, action: 'service.restart', title: 'Restart a service: nginx', state: 'lapsed',
    },
  ],
});

/** The SHA-256 fingerprint of the certificate the console serves, as `cert` prints it. */
export const FINGERPRINT = 'ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12';

/** `GET /api/1/console/about`, for a console that serves the certificate with `fingerprint`. */
export const about = (fingerprint = FINGERPRINT) => ({
  host: 'duke-nas', addresses: ['192.168.1.10'], port: 9090, fingerprint,
});

/** `GET /api/1/certificates/this-machine`: the machine's own certificate. */
export const machine = () => ({ name: 'this-machine', fingerprint_sha256: FINGERPRINT });

/** A token as authd answers it for a client that connects over HTTPS. Made up, and of no machine. */
export const TOKEN = 'dkc_Zm9yLXRlc3RzLW9ubHktbm90LWEtcmVhbC10b2tlbg';
