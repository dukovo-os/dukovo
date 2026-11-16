// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Websites pages say and send, as plain values: the names a site
// answers to and the rule they follow (`cert`'s), the two kinds of site, the
// certificates a site may have, the arguments of the plan and of each step of
// adding one, the rows of what that does, what stands when a step fails, and
// a site's HTTPS, state and hand edit in words. Nothing here touches the
// page. The addresses and the sites a certificate has are website-route.js's.

import { counted } from './size-words.js';

/** The most names one website answers to. */
export const NAMES_MOST = 20;

/** The most characters of a certificate's name, which a long first name cannot be. */
export const CERTIFICATE_NAME_MOST = 64;

/** The folder every website's folder is under. */
export const SITES_FOLDER = '/shared/www/sites';

/** The name of the certificate this server makes for itself. */
export const SELF = 'this-machine';

/** The apps a website needs: the web server, and PHP-FPM for PHP pages. */
export const NGINX_APP = 'org.nginx.nginx';
export const PHP_APP = 'net.php.php';

/** How long a firewall rule that opens a port is tried for, in seconds. */
export const PORT_TRY_SECONDS = 600;

/** The address a firewall test says a visitor from outside comes from: one made for documents. */
export const OUTSIDE = '203.0.113.1';

/** The two kinds of site the console adds, as the radio cards draw them. */
export const KINDS = Object.freeze([
  { id: 'static', title: 'Files from a folder', words: 'nginx serves the files as they are. A PHP file is never sent as text: it is not found.' },
  { id: 'php', title: 'PHP pages', words: 'nginx passes .php files to PHP-FPM, which runs them as the account php, for every PHP site of this server.' },
]);

/** The line that lets nginx read the sites' files, and what hides their folders from the default server: what nginx's settings need. */
export const INCLUDE_LINE = 'include /system/settings/websites/*.conf;';
export const HIDE_LINES = Object.freeze(['location ^~ /sites/ {', '    return 404;', '}']);

/** "a", "a and b", "a, b and c". */
export const listed = (items) => (items.length < 2 ? items.join('') : `${items.slice(0, -1).join(', ')} and ${items.at(-1)}`);

// ---- Names ----

const LABEL = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/;

/** The names typed in `text`: split at spaces, commas and line ends, in small letters, each once. */
export function namesOf(text) {
  const names = String(text ?? '').split(/[\s,]+/).map((name) => name.trim().toLowerCase()).filter(Boolean);
  return [...new Set(names)];
}

/** Whether `name` is an IPv4 address as Rust reads one: four numbers up to 255, none with a leading zero. */
const isIpv4 = (name) => {
  const parts = name.split('.');
  return parts.length === 4 && parts.every((part) => /^(?:0|[1-9]\d{0,2})$/.test(part) && Number(part) <= 255);
};

/**
 * What is wrong with `name` as a name a website answers to, as `cert` reads a
 * name for a certificate: lowercase letters, digits and `-` in words of up to
 * 63 characters with dots between them, 253 in all; no `*.` and no address by
 * number. A name of one word is let through (`localhost`). Empty when it is
 * right.
 */
export function nameProblem(name) {
  if (name.startsWith('*.')) return `${name} is a wildcard: a website answers to each name it is given, one by one.`;
  if (isIpv4(name) || name.includes(':')) return `${name} is an address: give the website a name, such as shop.example.org.`;
  if (name.length > 253 || !name.split('.').every((label) => LABEL.test(label))) {
    return `${name} is not a name a website can have: it has lowercase letters, digits and - in words of up to 63 characters, with a dot between them. Write a name that is not ASCII in punycode, as xn--…`;
  }
  return '';
}

/** What is wrong with the names `names`, or ''. */
export function namesProblem(names) {
  if (names.length === 0) return 'Give the website\'s domain, such as shop.example.org.';
  if (names.length > NAMES_MOST) return `A website has 1 to ${NAMES_MOST} names, not ${names.length}.`;
  for (const name of names) {
    const problem = nameProblem(name);
    if (problem) return problem;
  }
  return '';
}

/** The `www.` name of `name`, or null when it has one already or is a name of one word. */
export const wwwOf = (name) => (name.startsWith('www.') || !name.includes('.') ? null : `www.${name}`);

/** Whether the box for the `www.` name starts ticked: for a name that is not one of this server's own, `machineNames`. */
export const wwwDefault = (name, machineNames) => wwwOf(name) !== null && !machineNames.includes(name);

/** The names of the site: the domain, its `www.` name when asked, and `more`, each once. */
export function allNames({ domain, www, more }) {
  const first = domain === '' ? [] : [domain];
  const extra = www && wwwOf(domain) !== null ? [wwwOf(domain)] : [];
  return [...new Set([...first, ...extra, ...more])];
}

/** "the name", "both names", "all 3 names". */
export const bothWords = (names) => (names.length === 1 ? 'the name' : names.length === 2 ? 'both names' : `all ${names.length} names`);

// ---- The folder and who owns it ----

/** The folder a site of the name `name` has unless the person gives another. */
export const folderFor = (name) => `${SITES_FOLDER}/${name}`;

/** What is wrong with `folder`, which must be a folder under the sites' folder, or ''. */
export function folderProblem(folder) {
  const under = folder.startsWith(`${SITES_FOLDER}/`) ? folder.slice(SITES_FOLDER.length + 1) : null;
  if (under === null || under === '') return `The folder is under ${SITES_FOLDER}/, such as ${SITES_FOLDER}/shop.example.org.`;
  if (folder.length > 4096 || /[\0\n]/.test(folder) || under.split('/').some((part) => part === '' || part === '.' || part === '..')) {
    return 'The folder has no empty part, no . or .. and no line end in it.';
  }
  return '';
}

/** What the plan says of the folder, `{path, exists, entries, owner}`: when it is there, that its files are kept. */
export function folderWords(folder) {
  if (!folder?.exists) return '';
  const files = Number.isFinite(folder.entries) ? ` with ${counted(folder.entries, 'file')}` : '';
  return `The folder is there${files}: they are kept, and served as they are.`;
}

/** The accounts that may own a site's folder, from `GET /api/1/accounts`: those that are not disabled, the signed-in person first, then the administrators. */
export function ownerChoices(answer, self) {
  const list = Array.isArray(answer) ? answer : Array.isArray(answer?.accounts) ? answer.accounts : [];
  const rank = (one) => (one.name === self ? 0 : one.admin === true ? 1 : 2);
  return list.filter((one) => typeof one?.name === 'string' && one.name !== '' && one.disabled !== true)
    .sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name))
    .map((one) => one.name);
}

// ---- Certificates ----

/** The name of the certificate: the first name, or `short` when it is too long to be a certificate's name; null when neither is. */
export function certificateNameOf(names, short = '') {
  const first = names[0] ?? '';
  if (first !== '' && first.length <= CERTIFICATE_NAME_MOST) return first;
  return short.trim() === '' ? null : short.trim();
}

/** What is wrong with `name` as a certificate's name, as `cert` takes it, or ''. */
export function certificateNameProblem(name) {
  const fits = name.length >= 1 && name.length <= CERTIFICATE_NAME_MOST && /^[a-z0-9][a-z0-9.-]*$/.test(name) && !name.includes('..');
  return fits ? '' : `${name || 'An empty name'} is not a certificate's name: it has 1 to ${CERTIFICATE_NAME_MOST} lowercase letters, digits, - and ., starts with a letter or a digit, and has no ..`;
}

/** Whether a certificate that names `certificateNames` covers `name`: the name itself, or a wildcard of one word before the rest. */
export function covers(certificateNames, name) {
  return (certificateNames ?? []).some((one) => one === name
    || (one.startsWith('*.') && name.split('.').length === one.split('.').length && name.endsWith(one.slice(1))));
}

/** The names of this server's own certificate in `list` (`cert list --json`). */
export const machineNamesOf = (list) => (Array.isArray(list) ? list : []).find((one) => one.name === SELF)?.names ?? [];

/** The certificates of `list` that are not this server's own, are not revoked or out of time, and name every one of `names`. */
export function certificatesFor(names, list) {
  return (Array.isArray(list) ? list : []).filter((one) => one.name !== SELF && one.revoked !== true
    && !(Number.isFinite(one.days_left) && one.days_left < 0)
    && names.length > 0 && names.every((name) => covers(one.names, name)));
}

/** The words of each way a site may have its certificate. */
const WAYS = Object.freeze({
  letsencrypt: {
    title: 'From Let\'s Encrypt',
    words: 'Let\'s Encrypt checks that the names lead here, over port 80, then gives a certificate for 90 days; certd renews it.',
  },
  machine: {
    title: 'This machine\'s own',
    words: 'Browsers warn until they trust this machine\'s certificate; Certificates says how.',
  },
  there: {
    title: 'One that is here',
    words: 'A certificate certd has already, which names every name of the site.',
  },
  none: {
    title: 'None, HTTP only',
    words: 'Pages and passwords travel unencrypted.',
  },
});

/**
 * The ways `names` may have a certificate, as `{options, initial, here}`:
 * `options` are `{id, title, words, off, why, warn}` in the order they are
 * drawn, `initial` the id that starts chosen and `here` the certificates of
 * `list` that cover every name. Let's Encrypt gives certificates to names
 * with a dot, so that way is off for a name without one; this server's own
 * is offered only when every name is one of its own. `preset` names a
 * certificate the page was sent here to use.
 */
export function certificateChoices({ names, list, preset = null }) {
  const own = machineNamesOf(list);
  const allOwn = names.length > 0 && names.every((name) => own.includes(name));
  const here = certificatesFor(names, list);
  const dotless = names.find((name) => !name.includes('.'));
  const options = [
    {
      id: 'letsencrypt', ...WAYS.letsencrypt, off: dotless !== undefined, why: dotless === undefined ? '' : `Let's Encrypt gives certificates to names with a dot, and ${dotless} has none.`, warn: false,
    },
    ...(allOwn ? [{
      id: 'machine', ...WAYS.machine, off: false, why: '', warn: false,
    }] : []),
    ...(here.length > 0 ? [{
      id: 'there', ...WAYS.there, off: false, why: '', warn: false,
    }] : []),
    {
      id: 'none', ...WAYS.none, off: false, why: '', warn: true,
    },
  ];
  let initial;
  if (preset !== null && here.some((one) => one.name === preset)) initial = 'there';
  else if (allOwn) initial = 'machine';
  else if (dotless === undefined) initial = 'letsencrypt';
  else initial = here.length > 0 ? 'there' : 'none';
  return { options, initial, here };
}

/** The certificate a site that was chosen `how` has: its name, or null for none. `there` is the name chosen from the list, `short` the name given to a long first name. */
export function certificateOf({
  how, names, there = '', short = '',
}) {
  if (how === 'machine') return SELF;
  if (how === 'there') return there === '' ? null : there;
  return how === 'letsencrypt' ? certificateNameOf(names, short) : null;
}

// ---- What the page sends ----

/**
 * The arguments of `websites.plan` and `websites.add` for `form`: `{names,
 * kind, folder, owner, certificate, how}`. The other names go as one a line;
 * what is as it would be by default is left out. A certificate that is
 * got from Let's Encrypt is not named here: the site is added over HTTP
 * first, since the authority reaches it by port 80.
 */
export function siteArgs(form) {
  const [name, ...others] = form.names;
  const args = { name, kind: form.kind, owner: form.owner };
  if (others.length > 0) args.names = others.join('\n');
  if (form.folder !== folderFor(name)) args.folder = form.folder;
  if (form.how === 'machine' || form.how === 'there') args.certificate = form.certificate;
  return args;
}

/** The arguments of `certificate.get` for the names of `form`, once the terms are agreed. */
export function getArgs(form) {
  const args = { name: form.certificate, names: form.names.join('\n'), agree_terms: true };
  if (form.email.trim() !== '') args.email = form.email.trim();
  return args;
}

/** The arguments of the call that opens port `port` for a while. */
export const openPortArgs = (port, seconds = PORT_TRY_SECONDS) => ({
  action: 'allow', protocol: 'tcp', ports: String(port), try: seconds,
});

/** The arguments of `websites.https`: with a certificate, or `off`. */
export const httpsArgs = (name, certificate) => (certificate === null ? { name, off: true } : { name, certificate });

/** The arguments of `websites.remove`. */
export const removeArgs = (name, files) => (files ? { name, files: true } : { name });

/** The arguments of the firewall's test of a port as a visitor from outside meets it. */
export const testArgs = (port) => ({ protocol: 'tcp', port, from: OUTSIDE });

// ---- The steps ----

/** What the runner says of each action: its unlock words, the line while it runs, and the title of its error. */
const WORDS = Object.freeze({
  'apps.install': (subject) => ({ unlock: `install ${subject}`, running: `Installing ${subject}`, done: `${subject} is installed.`, failed: `${subject} was not installed` }),
  'websites.add': (subject) => ({ unlock: `add the website ${subject}`, running: `Adding ${subject}`, done: `${subject} is added.`, failed: `${subject} was not added` }),
  'certificate.get': (subject) => ({ unlock: `get a certificate for ${subject}`, running: `Getting a certificate for ${subject}`, done: `The certificate for ${subject} is got.`, failed: `The certificate for ${subject} was not got` }),
  'websites.https': (subject) => ({ unlock: `serve ${subject} over HTTPS`, running: `Turning on HTTPS for ${subject}`, done: `${subject} is served over HTTPS.`, failed: `HTTPS was not turned on for ${subject}` }),
});

/** What stands when a step fails, by the step: what the page says under it. */
export const STANDS = Object.freeze({
  install: () => 'Nothing about the website was changed.',
  add: () => 'Nothing was changed: the command took its changes back.',
  get: (site) => `${site} is live over HTTP. The certificate was not got: the site's page has Get the certificate to try again.`,
  https: (site, certificate) => `${site} is live over HTTP, and certd keeps the certificate ${certificate}. Try again turns HTTPS on.`,
});

/** One step, as lib/steps.js takes it: `id` is the page's own, to know which failed. */
const stepOf = (id, title, action, args, subject, stands, fix = '') => ({
  id, title, action, args, wait: 'task', words: { ...WORDS[action](subject), stands, ...(fix ? { fix } : {}) },
});

/** The step that installs `app` (`{id, name, sha256}`) first. */
export const installStep = (app) => stepOf('install', `Install ${app.name}`, 'apps.install', { id: app.id, sha256: app.sha256 }, app.name, STANDS.install());

/** The step that adds the site `form` describes. */
export const addStep = (form) => stepOf('add', `Add ${form.names[0]}`, 'websites.add', siteArgs(form), form.names[0], STANDS.add(), 'Put right what the message says, then try again.');

/** The step that gets the certificate: `names` and `certificate` as `getArgs` takes them. */
export const getStep = (form) => stepOf('get', `Get the certificate for ${form.names[0]}`, 'certificate.get', getArgs(form), form.names[0], STANDS.get(form.names[0]));

/** The step that serves the site `name` over HTTPS with `certificate`. */
export const httpsStep = (name, certificate) => stepOf('https', `Serve ${name} over HTTPS`, 'websites.https', httpsArgs(name, certificate), name, STANDS.https(name, certificate));

/** The step that serves the site `name` over HTTP only again; the certificate stays in certd. */
export const offStep = (name) => ({
  id: 'off',
  title: `Serve ${name} over HTTP only`,
  action: 'websites.https',
  args: httpsArgs(name, null),
  wait: 'task',
  words: {
    unlock: `serve ${name} over HTTP only`,
    running: `Turning HTTPS off for ${name}`,
    done: `${name} is served over HTTP only.`,
    failed: `HTTPS was not turned off for ${name}`,
    stands: `${name} is still served over HTTPS: nothing was changed.`,
  },
});

/** The step that removes the site `name`, and with `files` its folder. */
export const removeStep = (name, files) => ({
  id: 'remove',
  title: `Remove ${name}`,
  action: 'websites.remove',
  args: removeArgs(name, files),
  wait: 'task',
  words: {
    unlock: `remove the website ${name}`,
    running: `Removing ${name}`,
    done: `${name} is removed.`,
    failed: `${name} was not removed`,
    stands: `${name} is still there: nothing was changed.`,
  },
});

/**
 * The steps that add a site, in order. `missing` are the apps to install
 * first, as `{id, name, sha256}`. A certificate from Let's Encrypt is two
 * steps after the site, since the site answers on port 80 first.
 */
export function stepsOf(form, missing = []) {
  const steps = missing.map(installStep);
  steps.push(addStep(form));
  if (form.how === 'letsencrypt') steps.push(getStep(form), httpsStep(form.names[0], form.certificate));
  return steps;
}

/** The steps that give a site that is live over HTTP its certificate from Let's Encrypt, then HTTPS. */
export const certificateSteps = (form) => [getStep(form), httpsStep(form.names[0], form.certificate)];

/**
 * How a run of `steps` ended, as the page says it, from lib/steps.js's
 * result: `{ok, words, again, live}`. `again` is the index of the step "Try
 * again" starts at, or null when the page offers another way; `live` says
 * the site answers.
 */
export function outcomeOf(result, steps) {
  if (result.left) return { ok: false, words: '', again: null, live: false };
  if (result.done) return { ok: true, words: '', again: null, live: true };
  const failed = steps[result.failed];
  const live = failed.id === 'get' || failed.id === 'https';
  // A certificate that could not be got leaves the site live over HTTP: the site's page has the way to try again.
  return { ok: false, words: failed.words.stands, again: failed.id === 'get' ? null : result.failed, live };
}

/** The address a site that was added is at, from how it was given its certificate. */
export const liveUrl = (name, how) => `${how === 'none' ? 'http' : 'https'}://${name}/`;

// ---- What happens ----

/** The rows of the firewall, from the entries of the plan or of `firewall.test` (`{port, allowed, rule, text}`): one that says all is well, and one for each port a rule closes, with the button that opens it. */
export function firewallRows(entries) {
  const seen = new Set();
  const list = (Array.isArray(entries) ? entries : []).filter((one) => Number.isInteger(one?.port) && !seen.has(one.port) && seen.add(one.port));
  const rows = list.filter((one) => one.allowed === false).map((one) => ({
    key: `port-${one.port}`,
    icon: 'warn',
    tone: 'warn',
    words: Number.isInteger(one.rule) ? `Rule ${one.rule} closes port ${one.port}${one.text ? `: ${one.text}` : ''}` : `The firewall does not let port ${one.port} in`,
    port: one.port,
  }));
  const open = list.filter((one) => one.allowed !== false).map((one) => one.port).sort((a, b) => a - b);
  if (open.length > 0) {
    rows.unshift({
      key: 'firewall', icon: 'check', tone: 'ok', words: `Nothing to change: ${open.length === 1 ? `port ${open[0]} reaches` : `ports ${listed(open)} reach`} nginx (no rule closes ${open.length === 1 ? 'it' : 'them'})`, port: null,
    });
  }
  return rows;
}

/** How often a backup place copies, as a sentence names the backup. */
const EVERY = Object.freeze({ day: 'nightly backup', week: 'weekly backup', hand: 'backup' });

/** The row of the backups: whether a place copies /shared, which holds the sites. `backups` is `GET /api/1/backups`; null when it could not be read. */
export function backupRow(backups) {
  if (!Array.isArray(backups?.places)) return null;
  const place = backups.places.find((one) => Array.isArray(one.places) && (one.places.includes('/shared') || one.places.includes('/shared/www')));
  if (place) {
    return {
      key: 'backups', icon: 'check', tone: 'ok', words: `Nothing to change: /shared is in the ${EVERY[place.every] ?? 'backup'} to ${place.name}${place.every === 'hand' ? ', which is made by hand' : ''}`, link: null, port: null,
    };
  }
  return {
    key: 'backups', icon: 'warn', tone: 'warn', words: 'No backup place copies /shared yet', link: { href: '/snapshots/backups', words: 'Add a backup place' }, port: null,
  };
}

/** The row of the snapshots: whether snap.toml names /shared. `snapshots` is `GET /api/1/snapshots`; null when it could not be read. */
export function snapshotRow(snapshots) {
  const places = Array.isArray(snapshots?.settings?.places) ? snapshots.settings.places : (Array.isArray(snapshots?.places) ? snapshots.places.map((one) => one.place) : null);
  if (places === null) return null;
  const named = places.includes('/shared');
  return {
    key: 'snapshots',
    icon: named ? 'check' : 'warn',
    tone: named ? 'ok' : 'warn',
    words: named ? 'Snapshots: /shared has a snapshot every day' : 'Snapshots: snap.toml does not name /shared, so the sites have none',
    link: null,
    port: null,
  };
}

/**
 * The rows of What happens, `{key, icon, tone, words, port, link}`. `form`
 * is the page's, `plan` is `websites.plan`'s answer (null before it is
 * read, or while the console is locked), `world` is `{install, firewall,
 * backups, snapshots}`: the apps to install first as `{name}`, the
 * firewall's entries for the ports the plan did not ask about, and the
 * reads of the backups and snapshots.
 */
export function planRows(form, plan, world = {}) {
  const names = listed(form.names);
  const both = bothWords(form.names);
  const rows = [];
  const row = (key, iconName, words, more = {}) => rows.push({
    key, icon: iconName, tone: '', words, port: null, link: null, ...more,
  });
  for (const app of world.install ?? []) row(`install-${app.name}`, 'apps', `Installs ${app.name} first, from the catalog.`);
  if (form.how === 'letsencrypt') row('certificate', 'certs', `Gets a certificate for ${names} from Let's Encrypt; certd renews it by itself.`);
  else if (form.how === 'machine') row('certificate', 'certs', 'Serves it with this machine\'s own certificate. Browsers warn until they trust it.');
  else if (form.how === 'there') row('certificate', 'certs', `Serves it with the certificate ${form.certificate} that is here.`);
  else row('certificate', 'warn', 'Serves it over HTTP only: pages and passwords travel unencrypted.', { tone: 'warn' });
  const folder = plan?.folder ?? { path: form.folder, exists: null, owner: null };
  if (folder.exists === true) row('folder', 'storage', `Keeps ${folder.path} as it is${folder.owner ? `, owned by ${folder.owner}` : ''}.`);
  else row('folder', 'storage', `Makes ${folder.path}${folder.exists === false ? '' : ' if it is not there'} with a starter page, owned by ${form.owner}.`);
  if (form.how === 'none') row('site', 'globe', `Adds nginx's site for ${both}, over HTTP.`);
  else if (form.how === 'letsencrypt') row('site', 'globe', `Adds nginx's site for ${both}, then turns HTTPS on once the certificate is got: http:// then sends visitors to https://.`);
  else row('site', 'globe', `Adds nginx's site for ${both}, HTTPS only: http:// sends visitors to https://.`);
  if (plan?.restarts_nginx === true || form.how === 'letsencrypt') {
    row('restart', 'services', 'Restarts nginx once, to give it the certificate and port 443: Spark holds its ports, so no visitor is turned away.');
  }
  for (const name of plan?.turns_on ?? []) row(`on-${name}`, 'services', name === 'php-fpm' ? 'Turns PHP-FPM on.' : `Turns ${name} on.`);
  for (const one of firewallRows([...(plan?.firewall ?? []), ...(world.firewall ?? [])])) rows.push({ link: null, ...one });
  for (const one of [backupRow(world.backups), snapshotRow(world.snapshots)]) if (one) rows.push(one);
  return rows;
}

/** The checks of the plan that stop the add (`failed`, other than a missing app, which the first step installs) and those that only warn: `{blocking, warnings}`, each a list of `{id, words}`. */
export function planChecks(plan) {
  const checks = Array.isArray(plan?.checks) ? plan.checks : [];
  return {
    blocking: checks.filter((one) => one.result === 'failed' && one.id !== 'WEB-105'),
    warnings: checks.filter((one) => one.result === 'warning'),
  };
}

/** The ports a plan for `how` asks the firewall about: 80 always, and 443 when the site is served over HTTPS. */
export const portsFor = (how) => (how === 'none' ? [80] : [80, 443]);

/** The files of the plan, to preview: `[{path, state: 'new' | 'changes', lines: [{number, text}]}]`. */
export function previewFiles(plan) {
  return (Array.isArray(plan?.files) ? plan.files : []).filter((file) => typeof file?.path === 'string').map((file) => ({
    path: file.path,
    state: file.new === true ? 'new' : 'changes',
    lines: String(file.text ?? '').replace(/\n$/, '').split('\n').map((text, at) => ({ number: at + 1, text })),
  }));
}

// ---- The list and a site ----

/** The Serves column: "Files" or "PHP pages", or what a site the console did not make does. */
export const kindWords = (site) => (site.kind === 'php' ? 'PHP pages' : site.kind === 'static' ? 'Files' : 'Set up by hand');

/** What a site serves, in a sentence: "PHP pages from /shared/www/sites/x, owned by webdev". */
export function serveWords(site) {
  const from = site.folder ? ` from ${site.folder}` : '';
  const owner = site.owner ? `, owned by ${site.owner}` : '';
  return site.kind === 'other' ? `Set up by hand${from}` : `${kindWords(site)}${from}${owner}`;
}

/** The state of a site: `{tone, words, id}`, from nginx's and PHP-FPM's states and the site's own problems. */
export function stateOf(site, { nginx, phpFpm } = {}) {
  if (nginx?.installed === false) return { tone: 'bad', words: 'nginx is not installed', id: null };
  if (nginx?.running === false) return { tone: 'bad', words: 'nginx is off', id: null };
  const first = site.problems?.[0];
  if (first) return { tone: 'warn', words: first.words, id: first.id ?? null };
  if (site.kind === 'php' && phpFpm?.installed === false) return { tone: 'warn', words: 'PHP-FPM is not installed: its PHP pages fail', id: null };
  if (site.kind === 'php' && phpFpm?.running === false) return { tone: 'warn', words: 'PHP-FPM is off: its PHP pages fail', id: null };
  return { tone: 'ok', words: 'Answers', id: null };
}

/**
 * The words of a site's HTTPS, `{words, tone, certificate, ends, renews}`,
 * from its certificate and what `cert list --json` says of it. `date(text)`
 * gives a date as words from the ISO text.
 */
export function httpsOf(site, list, date) {
  const none = { certificate: null, ends: null, renews: null };
  if (!site.https) return { ...none, words: 'HTTP only', tone: 'warn' };
  if (!site.certificate) return { ...none, words: 'HTTPS, with a certificate certd does not keep', tone: 'warn' };
  // Without the list of certificates all that is known is the name.
  if (!Array.isArray(list)) return { ...none, certificate: site.certificate, words: `Certificate ${site.certificate}`, tone: '' };
  const found = list.find((one) => one.name === site.certificate);
  if (!found) return { ...none, certificate: site.certificate, words: `Certificate ${site.certificate}, which certd does not have`, tone: 'warn' };
  const base = {
    certificate: found.name, tone: found.revoked === true ? 'warn' : '', ends: found.not_after ? date(found.not_after) : null, renews: found.renews ? date(found.renews) : null,
  };
  if (found.name === SELF || found.kind === 'self-signed') return { ...base, words: 'This machine\'s certificate' };
  if (found.kind === 'added') return { ...base, words: `Certificate ${found.name} (added)` };
  if (found.revoked === true) return { ...base, words: `Let's Encrypt · ${found.name} is revoked` };
  return { ...base, words: `Let's Encrypt · ${base.renews ? `renews ${base.renews}` : `ends ${base.ends}`}` };
}

/** The line of a site's file: `{words, tone, by}` for a file changed by hand, made by hand, or as it was written. */
export function handEditWords(site) {
  if (site.added_by_hand) return { tone: 'warn', by: 'added', words: 'Added by hand · website does not write it, and keeps it as you wrote it' };
  if (site.by_hand) return { tone: 'warn', by: 'changed', words: 'Changed by hand · kept as you wrote it' };
  return { tone: '', by: null, words: 'As website wrote it' };
}

/** Why the console does not change the HTTPS of `site`, or ''. */
export function httpsLocked(site) {
  if (site.kind === 'other') return 'nginx serves this site by a file made by hand, which website does not change: add HTTPS to it yourself.';
  if (site.by_hand || site.added_by_hand) return 'The file was changed by hand, so website does not write it: add the HTTPS server yourself, as website(8) shows.';
  return '';
}

/** Whether taking HTTPS from `site` (removing it, or turning HTTPS off) restarts nginx: no other site uses its certificate, so nginx's service file loses it. */
export function lastToUse(site, sites) {
  if (!site.https) return false;
  return !sites.some((one) => one.name !== site.name && one.https && one.certificate === site.certificate);
}

const RESTARTS = 'nginx restarts once, since no other site uses this certificate: Spark holds its ports, so no visitor is turned away.';
const RELOADS = 'nginx reads its settings again, and no visitor is turned away.';

/** What Remove… says it does, as the lead of its dialog. */
export function removeLead(site, sites) {
  const first = site.names?.[0] ?? site.name;
  const folder = site.folder ? ' Its folder is kept unless you tick the box below.' : '';
  const certificate = site.https && site.certificate ? ' Its certificate stays in certd.' : '';
  return [`Removes nginx's site for ${first}.${folder}${certificate}`, lastToUse(site, sites) ? RESTARTS : RELOADS];
}

/** What turning HTTPS off says: the certificate stays in certd. */
export function offLead(site, sites) {
  const first = site.names?.[0] ?? site.name;
  return [`${first} is served over HTTP only afterwards, and pages and passwords travel unencrypted. The certificate stays in certd.`, lastToUse(site, sites) ? RESTARTS : RELOADS];
}

/** The words of the box that also removes the folder. */
export const folderBoxWords = (site) => `Also remove its folder ${site.folder} and everything in it`;

// ---- Check the sites ----

/** The words of what `websites.check` found of nginx's `nginx.conf`: `[{key, tone, id, words, lines}]`, nothing when it has both lines. */
export function confRows(check) {
  const conf = check?.nginx_conf;
  if (!conf) return [];
  const rows = [];
  if (conf.includes === false) {
    rows.push({
      key: 'include', tone: 'bad', id: 'WEB-102', words: `nginx's settings do not include the websites' folder (${conf.path ?? 'nginx.conf'}). Add this line at the end of its http block:`, lines: [INCLUDE_LINE],
    });
  }
  if (conf.hides_sites === false) {
    rows.push({
      key: 'hides', tone: 'warn', id: 'WEB-102', words: 'The default server does not hide /sites/, so a site\'s folder is served there too. Add this inside the default server, before location /:', lines: [...HIDE_LINES],
    });
  }
  return rows;
}

/** nginx's check of the sites as they are: `{passed, tone, words, line}`, or null when the answer has none. */
export function checkWords(check) {
  if (!check?.check) return null;
  if (check.check.passed) return { passed: true, tone: 'ok', words: 'nginx\'s check of the sites passed.', line: null };
  return {
    passed: false, tone: 'bad', words: 'nginx\'s check of the sites failed, so nothing is changed until it passes.', line: check.check.line ?? null,
  };
}

/** The problems `websites.check` found for each site: `{site: [{id, words}]}`. */
export const checkedProblems = (check) => Object.fromEntries((Array.isArray(check?.websites) ? check.websites : []).map((one) => [one.name, one.problems ?? []]));

/** The lines above the list when nginx or PHP-FPM does not run, `[{key, words, enable, sheet}]`: `enable` is the service Turn it on starts, `sheet` the app's page when it is not installed. */
export function serverLines(answer, sites) {
  const lines = [];
  const nginx = answer?.nginx;
  if (nginx?.installed === false) {
    lines.push({
      key: 'nginx', words: 'nginx is not installed: no site answers.', enable: null, sheet: `/apps/add/${NGINX_APP}`,
    });
  } else if (nginx && nginx.running === false) {
    lines.push({
      key: 'nginx', words: 'nginx is off: no site answers.', enable: 'nginx', sheet: null,
    });
  }
  const php = answer?.php_fpm;
  if (php && sites.some((site) => site.kind === 'php')) {
    if (php.installed === false) {
      lines.push({
        key: 'php-fpm', words: 'PHP-FPM is not installed: its PHP pages fail.', enable: null, sheet: `/apps/add/${PHP_APP}`,
      });
    } else if (php.running === false) {
      lines.push({
        key: 'php-fpm', words: 'PHP-FPM is off: its PHP pages fail.', enable: 'php-fpm', sheet: null,
      });
    }
  }
  return lines;
}

/** The apps the kind needs that `answer` (`GET /api/1/websites`) says are not installed, `[{id, name}]`. */
export function missingApps(answer, kind) {
  const missing = [];
  if (answer?.nginx?.installed === false) missing.push({ id: NGINX_APP, name: 'nginx' });
  if (kind === 'php' && answer?.php_fpm?.installed === false) missing.push({ id: PHP_APP, name: 'PHP-FPM' });
  return missing;
}

/** The words that say who may change what. */
export const READER_WORDS = 'Only an administrator can add, change or remove a website.';
