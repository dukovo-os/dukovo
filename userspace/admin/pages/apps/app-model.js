// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What an app's page says, as plain values: the line under its name, the
// flaws and the update in its banner, its services, what it may use, provides,
// uses and what uses it, its data, its versions, and the words of the
// questions that going back and removing ask.

import { bytesWords, counted } from '../../lib/size-words.js';
import { appHref, capital, isText, listOf, urgent, worstOf } from './apps-model.js';

/** The line under an app's name: where it came from, that it is signed, and its licence. */
export function byline(info) {
  const trust = info.trust ?? {};
  const signed = trust.kind === 'developer' ? 'signed by its developer\'s key, trusted by hand' : 'signed';
  let source = null;
  if (trust.kind === 'catalog' && isText(trust.catalog)) source = `${capital(trust.catalog)} catalog`;
  else if (isText(info.update_feed)) source = 'its own feed';
  else if (trust.kind === 'bound') source = 'a file you installed';
  else if (trust.kind === 'bound_for_everyone') source = 'a file installed for everyone';
  return [source, signed, isText(info.license) ? info.license : null].filter(Boolean).join(' · ');
}

/** The numbers of a version that is nothing but dotted numbers, or null. */
const numbersOf = (version) => (/^v?\d+(?:\.\d+)*$/.test(String(version ?? '').trim()) ? String(version).trim().replace(/^v/, '').split('.').map(Number) : null);

/** Whether `version` is `wanted` or later, when both are dotted numbers; false when either is not. */
function reaches(version, wanted) {
  const have = numbersOf(version);
  const need = numbersOf(wanted);
  if (!have || !need) return false;
  for (let at = 0; at < Math.max(have.length, need.length); at += 1) {
    const [one, other] = [have[at] ?? 0, need[at] ?? 0];
    if (one !== other) return one > other;
  }
  return true;
}

/**
 * The known flaws of the installed version, from `info.flaws.known`, each
 * `{id, severity, component, summary, fixed, byOffer}`: `fixed` is the
 * version of its component that fixes it, and `byOffer` says that the
 * offered version `to` is that one or later.
 */
export function flawsOf(info, to) {
  return listOf(info.flaws?.known).filter((flaw) => isText(flaw?.id)).map((flaw) => ({
    id: flaw.id,
    severity: flaw.severity,
    component: isText(flaw.component) ? flaw.component : null,
    summary: isText(flaw.summary) ? flaw.summary : null,
    fixed: isText(flaw.fixed) ? flaw.fixed : null,
    byOffer: isText(to) && reaches(to, flaw.fixed),
  }));
}

/** What an app's flaws say when it has none to list: the catalog's silence, or that none is known; null when `bundle info` says nothing of them. */
export function noFlawsWords(info) {
  if (!info.flaws) return null;
  if (info.flaws.state === 'unknown') return 'The catalog says nothing of this version\'s flaws.';
  return listOf(info.flaws.known).length === 0 ? 'None known.' : null;
}

/** Whether an update newer than the installed version is known, offered or held back. */
export const isNewer = (update) => update.state === 'update' || update.state === 'waiting' || (update.state === 'held' && isText(update.to));

/** How each state of an update starts the banner's first sentence. */
const LEADS = Object.freeze({
  update: (update) => `${update.to} is available (${update.kind})`,
  waiting: (update) => `${update.to} waits for its wave`,
  held: (update, info) => `${update.to} is available, and ${info.version} is held`,
});

/**
 * What sits above an app's facts: the offered version and the known flaws of
 * the installed one, as `{kind, heading, flaws}` with `kind` the banner's
 * colour, or null when there is neither. `update` is the row's update.
 */
export function bannerOf(info, update) {
  const newer = isNewer(update);
  const flaws = flawsOf(info, newer ? update.to : null);
  if (flaws.length === 0 && !newer) return null;
  const worst = worstOf(flaws.map((flaw) => flaw.severity));
  const sentences = [];
  if (newer) sentences.push(LEADS[update.state](update, info));
  if (flaws.length > 0) {
    const fixed = flaws.filter((flaw) => flaw.byOffer).length;
    const known = `${info.version} has ${counted(flaws.length, 'known flaw')}${worst ? `, worst ${worst}` : ''}`;
    const fixes = fixed > 0 ? `; ${update.to} fixes ${fixed === flaws.length ? 'all of them' : `${fixed} of them`}` : '';
    sentences.push(`${known}${fixes}${newer ? '' : ', and the catalog lists no newer version yet'}`);
  }
  let kind = 'info';
  if (flaws.length > 0) kind = urgent(worst) ? 'bad' : 'warn';
  return { kind, heading: `${sentences.join('. ')}.`, flaws };
}

/** One line for a flaw of the banner: "DKSA-2026-0007 · high · valkey 8.0.3: a crafted command stops the server". */
export function flawLine(flaw) {
  const place = flaw.component ? `${flaw.component}: ` : '';
  return `${[flaw.id, flaw.severity].filter(isText).join(' · ')} · ${place}${flaw.summary ?? 'no summary'}`;
}

/** Where a flaw is fixed: its version, and that the offered version brings it; or that no fix is known. */
export const fixedWords = (flaw) => (flaw.fixed ? `Fixed in ${flaw.fixed}${flaw.byOffer ? ', which the update brings' : ''}.` : 'No fix is known yet.');

/** What the banner adds when flaws are listed: the page does not judge the risk. */
export const RISK_WORDS = 'Whether a flaw matters here depends on what the app may use, which is listed below.';

/** The address of the page of the service `name`; a dot is written out, as for an app. */
export const serviceHref = (name) => `/services/${encodeURIComponent(name).replaceAll('.', '%2E')}`;

/** What each state Spark names is called, and the colour that repeats the word. */
const SERVICE_STATES = Object.freeze({
  running: ['Running', 'ok'], listening: ['Ready', 'ok'], starting: ['Starting', 'warn'], stopping: ['Stopping', 'warn'],
  failed: ['Failed', 'bad'], done: ['Done', 'ok'], waiting: ['Waiting', 'warn'], pending: ['Waiting', 'off'],
  stopped: ['Stopped', 'off'], disabled: ['Stopped', 'off'],
});

/** The state `id` as `{word, kind}`; a state this page does not know is its own word with no colour, and none is null. */
function stateWords(id) {
  if (Object.hasOwn(SERVICE_STATES, id)) return { word: SERVICE_STATES[id][0], kind: SERVICE_STATES[id][1] };
  return isText(id) ? { word: capital(id), kind: 'off' } : null;
}

/** How a service of Spark's list starts by itself, in words after its state: "off at boot". */
function bootWords(service) {
  if (service.enabled !== true) return 'off at boot';
  if (service.timer === true) return 'by its timer';
  return service.start === 'on-connection' ? 'on connection' : 'starts at boot';
}

/**
 * The services of the app, for each of `service_list`: `{name, account, href,
 * state, boot}`. `state` is `{word, kind}` from Spark's list `show` (what
 * `GET /api/1/services` answers, a list or a document with `services`, or
 * null when it could not be read), and null when Spark does not know the
 * service; `boot` is how it starts, from Spark's list, else from the
 * manifest's `start`, in words that follow the state ("off at boot"). Null
 * when `bundle info` gives no list (an older one gives each service in a
 * line, `services`).
 */
export function servicesOf(info, show = null) {
  if (!Array.isArray(info.service_list)) return null;
  const known = new Map(listOf(Array.isArray(show) ? show : show?.services).filter((service) => isText(service?.name)).map((service) => [service.name, service]));
  return info.service_list.filter((service) => isText(service?.name)).map((service) => {
    const found = known.get(service.name) ?? null;
    return {
      name: service.name,
      account: isText(service.account) ? service.account : service.name,
      href: serviceHref(service.name),
      state: found ? stateWords(found.state) : null,
      boot: found ? bootWords(found) : service.start === 'on' ? 'starts at boot' : 'off at boot',
    };
  });
}

/** The lines `bundle info` gives for its services when it gives no list. */
export const serviceLines = (info) => listOf(info.services).filter(isText);

/** "its own account, valkey": the accounts the services run as, each once. */
export function runsAs(services) {
  const accounts = [...new Set(services.map((service) => service.account))];
  if (accounts.length === 0) return null;
  return `${accounts.length === 1 ? 'its own account' : 'its own accounts'}, ${accounts.join(', ')}`;
}

/** What a permission lets the app do, with its folders, ports and apps: each a line of the "May use" row. */
export function mayUseOf(info) {
  return listOf(info.permissions).filter((permission) => permission && (isText(permission.lets) || isText(permission.name))).map((permission) => {
    const folders = listOf(permission.folders).filter((one) => isText(one?.folder)).map((one) => `${one.folder}${isText(one.access) ? ` (${one.access})` : ''}`);
    const ports = listOf(permission.ports).map(String);
    const apps = listOf(permission.apps).filter(isText);
    return [
      isText(permission.lets) ? permission.lets : permission.name,
      folders.length > 0 && `folders ${folders.join(', ')}`,
      ports.length > 0 && `${ports.length === 1 ? 'port' : 'ports'} ${ports.join(', ')}`,
      apps.length > 0 && `with ${apps.join(', ')}`,
    ].filter(Boolean).join(' · ');
  });
}

/** What an interface or a socket that the app provides or uses is called: "fastcgi at /run/php-fpm.sock". */
export const linkWords = (link) => [link?.interface, link?.socket && `at ${link.socket}`].filter(isText).join(' ');

/** The links of `info[name]` (`provides` or `uses`), or null when `bundle info` gives none of that name, which an older one does not. */
export function linksOf(info, name) {
  if (!Array.isArray(info[name])) return null;
  const links = info[name].map(linkWords).filter(isText);
  return links.length > 0 ? links : null;
}

/** The installed apps that use this one, `{id, name, what, href}`; null when `bundle info` does not say, which an older one does not. */
export function usedByOf(info) {
  if (!Array.isArray(info.used_by)) return null;
  return info.used_by.filter((one) => isText(one?.id)).map((one) => ({
    id: one.id, name: isText(one.name) ? one.name : one.id, what: isText(one.what) ? one.what : null, href: appHref(one.id),
  }));
}

/** The data folder of each service of the app, which ADR 0385 puts at `/shared/services/ID/ACCOUNT`. */
export const dataFolders = (info, services) => [...new Set(services.map((service) => `/shared/services/${info.id}/${service.account}`))];

/** The size of the app, "9 MB", or null when it is not told. */
export const sizeWords = (info) => (Number.isFinite(info.bytes) ? bytesWords(info.bytes) : null);

/** How the apps update, from `updates.settings` (`update settings --json`): "Update by themselves", "Tell me", or null when it is not told. */
export function ruleWords(settings) {
  if (settings?.apps === 'auto') return 'Update by themselves';
  return settings?.apps === 'notify' ? 'Tell me' : null;
}

/** The seconds of a time given as seconds or as an RFC 3339 text, or null. */
const secondsOf = (stamp) => {
  if (Number.isFinite(stamp)) return Math.floor(stamp);
  const ms = Date.parse(stamp);
  return Number.isFinite(ms) ? Math.floor(ms / 1000) : null;
};

/**
 * The table of versions: `{version, note, pill, back}` for the offered one,
 * the installed one and the one kept for going back. `time` is the console's
 * (lib/time.js).
 */
export function versionsOf(info, update, time) {
  const rows = [];
  if (isNewer(update)) {
    const pill = update.state === 'held' ? { words: 'held back', kind: 'warn' } : { words: 'available', kind: 'info' };
    rows.push({ version: update.to, note: capital(update.kind), pill, back: false });
  }
  const installed = secondsOf(info.installed);
  rows.push({
    version: info.version,
    note: installed === null ? null : `Installed ${time.relative(installed)}`,
    pill: info.held ? { words: 'held', kind: 'warn' } : { words: 'installed', kind: 'ok' },
    back: false,
  });
  if (isText(info.previous)) rows.push({ version: info.previous, note: 'Kept for going back', pill: null, back: true });
  return rows;
}

/** What is said when no earlier version is kept. */
export const NO_PREVIOUS = 'No earlier version is kept yet: an update keeps the one it replaces.';

/** What going back says before it goes. */
export const rollbackWords = (info) => `Goes back to ${info.previous} and holds it there. Its data comes back as ${info.previous} left it: its services' data at once, the rest at its next run (bundle(1)). Data written since the update is kept in its snapshot for 30 days.`;

/** What holding says under the buttons of a held app. */
export const heldWords = (info) => `Held at ${info.version}: no update is taken, security fixes included. Release the hold to take updates again.`;

/** What a reader is told in place of the buttons. */
export const READER_LINE = 'An administrator can update, hold, go back and remove apps.';

/** The apps that use this one, as sentences for the remove dialog: "php-fpm uses it: removing it stops php-fpm's sessions." */
export function usersWords(usedBy) {
  return usedBy.map((one) => (one.what
    ? `${one.name} uses it: removing it stops ${one.name}'s ${one.what}.`
    : `${one.name} uses it, and may stop working without it.`));
}

/** The two answers of the remove dialog's question, in the order shown, and the value of each that `apps.remove` takes. */
export const REMOVE_CHOICES = Object.freeze([
  { value: 'keep', label: 'Keep its data', words: 'Its settings and data stay where they are, and a later version of the app picks them up.' },
  { value: 'purge', label: 'Remove its data too', words: 'Its settings and data go, with the copies kept for going back. This cannot be undone.' },
]);

/** What the remove dialog says first. */
export const removeLead = (info) => `${info.name} ${info.version} is removed for everyone, and its services stop and go.`;

/** What the page says over the sheet of an update that stopped to ask. */
export const ASKED_WORDS = 'This update asks for more than the version installed does. Read what it asks for before you go on.';
