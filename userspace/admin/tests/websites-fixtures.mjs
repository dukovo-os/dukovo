// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Answers in the shapes of `website list --json`, `website check --json` and
// `website add --dry-run --json`, and of the reads the Websites pages make
// besides (`cert list --json`, `user list --json`, `backup list --json`,
// `snap list --json`, `hostnamectl status --json`, `bundle sheet --json`),
// for the tests of the Websites pages. A small server: a PHP shop over
// HTTPS, a blog over HTTP, a site for the machine's own name and one made by
// hand. Not a test itself, and never installed.

import { account } from './accounts-fixtures.mjs';
import { list as certificateList, machine, NOW, time } from './certificates-fixtures.mjs';
import { backupsOf, current, place } from './snapshots-fixtures.mjs';

export {
  machine, NOW, time,
};

/** The text of a site's file, as `website` writes one. */
export const fileText = (name) => `# website: ${name}\nserver {\n    listen 80;\n    server_name ${name};\n    root /shared/www/sites/${name};\n}\n`;

/** One site of `website list --json`, with what a test says over it. */
export const site = (name, more = {}) => ({
  name,
  names: [name],
  file: `/system/settings/websites/${name}.conf`,
  kind: 'static',
  folder: `/shared/www/sites/${name}`,
  owner: 'webdev',
  https: false,
  certificate: null,
  certificate_path: null,
  redirects_http: false,
  by_hand: false,
  added_by_hand: false,
  problems: [],
  text: fileText(name),
  ...more,
});

/** The state of a program of a read. */
const state = (running = true, installed = true) => ({ installed, enabled: running, running });

/** `website list --json` for the small server. */
export const list = (more = {}) => ({
  format: 1,
  nginx: state(),
  php_fpm: state(),
  websites: [
    site('shop.example.org', {
      names: ['shop.example.org', 'www.shop.example.org'], kind: 'php', https: true, certificate: 'shop.example.org', redirects_http: true,
    }),
    site('blog.example.org', { owner: 'chase' }),
    site('duke-nas.local', {
      names: ['duke-nas.local'], https: true, certificate: 'this-machine', redirects_http: true,
    }),
    site('legacy', {
      names: ['legacy.example.org'], kind: 'other', folder: null, owner: null, by_hand: true, added_by_hand: true, text: 'server { listen 80; server_name legacy.example.org; proxy_pass http://127.0.0.1:8080; }\n',
    }),
  ],
  ...more,
});

/** No site yet, nginx on. */
export const empty = () => list({ websites: [] });

/** nginx is not installed, and PHP-FPM is not. */
export const missing = () => list({ nginx: state(false, false), php_fpm: state(false, false), websites: [] });

/** nginx is installed and off, with a site of each kind. */
export const off = () => list({ nginx: state(false) });

/** PHP-FPM is off while a PHP site is there. */
export const phpOff = () => list({ php_fpm: state(false) });

/** One site with a problem anyone can see. */
export const problem = () => list({
  websites: [site('shop.example.org', { kind: 'php', problems: [{ id: null, words: 'Its folder /shared/www/sites/shop.example.org is not there' }] })],
});

/** `website check --json` when all is well. */
export const checkOk = () => ({
  format: 1, nginx_conf: { path: '/shared/services/org.nginx.nginx/nginx/conf/nginx.conf', includes: true, hides_sites: true }, check: { passed: true, line: null }, websites: [{ name: 'shop.example.org', problems: [] }],
});

/** `website check --json` for settings that lack both lines and a file nginx refuses. */
export const checkBroken = () => ({
  format: 1,
  nginx_conf: { path: '/shared/services/org.nginx.nginx/nginx/conf/nginx.conf', includes: false, hides_sites: false },
  check: { passed: false, line: 'nginx: [emerg] unknown directive "listn" in /system/settings/websites/blog.example.org.conf:3' },
  websites: [
    { name: 'blog.example.org', problems: [{ id: 'WEB-103', words: 'nginx refuses its file: unknown directive "listn"' }] },
    { name: 'shop.example.org', problems: [] },
  ],
});

/** The plan of a site that is new: the folder is not there, a file is to be written. */
export const plan = (more = {}) => ({
  format: 1,
  checks: [
    { id: null, result: 'passed', words: 'No site answers to shop.example.org' },
    { id: null, result: 'passed', words: 'nginx is installed' },
  ],
  folder: {
    path: '/shared/www/sites/shop.example.org', exists: false, entries: 0, owner: null,
  },
  files: [
    { path: '/system/settings/websites/shop.example.org.conf', new: true, text: fileText('shop.example.org') },
    { path: '/shared/www/sites/shop.example.org/index.php', new: true, text: '<?php\necho "shop.example.org works.";\n' },
  ],
  steps: [
    { words: 'Make the folder', argv: ['mkdir', '/shared/www/sites/shop.example.org'] },
    { words: 'Write the site', argv: ['website', 'add'] },
  ],
  restarts_nginx: false,
  turns_on: [],
  firewall: [{ port: 80, allowed: true, rule: null, text: '' }],
  nginx_check: 'passed',
  nginx_line: null,
  ...more,
});

/** `firewall test --json`: allowed, with no rule that decides. */
export const open = () => ({ allowed: true, rule: null, text: '', passed: [] });

/** `firewall test --json`: a rule that closes the port. */
export const closed = () => ({ allowed: false, rule: 3, text: 'deny tcp 443', passed: [] });

/** `user list --json`: people, one disabled. */
export const accounts = () => [
  account('webdev', { uid: 1001 }),
  account('chase', { uid: 1000, admin: true }),
  account('old', { uid: 1004, disabled: true }),
];

/** `backup list --json` with one place that copies /shared every night. */
export const backups = () => backupsOf(place('nas'));

/** `backup list --json` with a place that does not copy /shared. */
export const backupsWithout = () => backupsOf(place('nas', { places: ['/users'] }));

/** `snap list --json`, with snapshots of /shared. */
export const snapshots = () => current();

/** `cert list --json` for the small server: this machine's own, the shop's, a wildcard and others. */
export const certificates = () => certificateList();

/** `bundle sheet ID --json`: the part of it the page reads. */
export const sheet = (id, more = {}) => ({
  id, name: id.split('.').at(-1), sha256: id.startsWith('org.nginx') ? 'a'.repeat(64) : 'b'.repeat(64), ...more,
});

/** A task that ended well. */
export const finished = (id, log = ['website: added shop.example.org (php, HTTPS)']) => ({
  id, state: 'done', step: 5, steps: 5, words: '', log, changed: true,
});

/** A task that ended badly, with the line the command ended with. */
export const failedTask = (id, line) => ({
  id, state: 'failed', status: 1, step: 2, steps: 5, words: '', log: [line], changed: false,
});

/** The path a read of the plan of `name` is asked at. */
export const planPath = (name) => `/api/1/websites/plan/${encodeURIComponent(name)}`;
