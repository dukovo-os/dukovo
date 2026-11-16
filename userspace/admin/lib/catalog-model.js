// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the pages that add an app say, as plain values so that a test reaches
// every branch: the words of a search and the rows it finds, the banners of
// catalogs that could not be read, the ready-made setups, the sheet of an app
// before it is installed, what stands after the install, and the next step.
// The words for flaws, permissions, services and links are those of the
// Apps pages (pages/apps/app-model.js); this file adds what only a sheet of an
// app that is not installed yet says.

import {
  appHref, capital, flawWords, isText, listOf, urgent, worstOf,
} from '../pages/apps/apps-model.js';
import {
  dataFolders, fixedWords, flawLine, flawsOf, isPermission, linkWords, NGINX_ID, noFlawsWords, permissionLine, POSTGRESQL_ID,
  servicesOf, signedWords,
} from '../pages/apps/app-model.js';
import { initialOf, tintOf } from '../pages/apps/mark.js';
import { bytesWords } from './size-words.js';

/** The bundle that makes PHP pages, whose service is php-fpm. */
export const PHP_ID = 'net.php.php';

// ---- the search ---------------------------------------------------------------

/** The most words a search takes, and the shape of each: what `apps.search` checks. */
export const MAX_WORDS = 5;
const WORD = /^[a-z0-9.@-]{1,64}$/;

/**
 * What was typed in the search, as the words `apps.search` takes: lowercase,
 * split at spaces. Gives `{words, problem}`: `problem` says why the search
 * cannot be sent (more than five words, or a character no ID or name has), and
 * then `words` is empty.
 */
export function wordsOf(text) {
  const words = String(text ?? '').toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length > MAX_WORDS) return { words: [], problem: `Search for at most ${MAX_WORDS} words at a time.` };
  if (!words.every((word) => WORD.test(word))) return { words: [], problem: 'Words of a search are letters, digits, dots, @ and dashes.' };
  return { words, problem: null };
}

/** The address that reads the bundles the catalogs list for `words` (none: every bundle). */
export const searchPath = (words) => `/api/1/apps/catalog/search${words.length > 0 ? `?words=${encodeURIComponent(words.join(' '))}` : ''}`;

/** The address that reads the sheet of the app `id`. */
export const sheetPath = (id) => `/api/1/apps/catalog/${encodeURIComponent(id)}/sheet`;

/** The page that adds an app, with what is typed in its search kept in the address. */
export const addHref = (typed = '') => `/apps/add${typed === '' ? '' : `?q=${encodeURIComponent(typed)}`}`;

/** The address of the sheet of the app `id`: a dot is written out, as for an app's page. */
export const sheetHref = (id, typed = '') => `/apps/add/${encodeURIComponent(id).replaceAll('.', '%2E')}${typed === '' ? '' : `?q=${encodeURIComponent(typed)}`}`;

/** The command that installs `id` for everyone, which asks its question at the terminal. */
export const installCommand = (id) => `elevate bundle install ${id} --for-everyone`;

/** What a reader is told in place of Install. */
export const READER_WORDS = 'An administrator installs apps for everyone.';

// ---- the rows of the catalogs ---------------------------------------------------

/** What the Flaws cell says of `flaws` (`{state, count, worst}`): the words, and whether they are in the warning colour. */
function flawsCell(flaws) {
  const known = flawWords(flaws);
  if (known !== null) return { words: known, urgent: urgent(worstOf([flaws.worst])) };
  if (flaws?.state === 'none' || flaws?.state === 'known') return { words: 'No known flaws', urgent: false };
  return { words: 'Flaws not listed', urgent: false };
}

/** Whether `a` comes before `b`: by name, as people read it, then by ID. */
const byName = (a, b) => {
  const [one, other] = [a.name.toLowerCase(), b.name.toLowerCase()];
  if (one !== other) return one < other ? -1 : 1;
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
};

/**
 * The rows of the table of the catalogs, from `apps.search`'s answer and the
 * apps that are installed (`apps.list`'s `bundles`), sorted by name:
 * `{id, name, version, catalog, href, initial, tint, flaws, installed}`.
 * `installed` is the version installed, from the search's own member or from
 * the list, else null; `typed` is kept in each row's address.
 */
export function rowsOf(answer, apps = [], typed = '') {
  const here = new Map(listOf(apps).filter((one) => isText(one?.id) && isText(one.version)).map((one) => [one.id, one.version]));
  return listOf(answer?.bundles).filter((one) => isText(one?.id)).map((one) => {
    const name = isText(one.name) ? one.name : one.id;
    return {
      id: one.id,
      name,
      version: isText(one.version) ? one.version : '',
      catalog: isText(one.catalog) ? one.catalog : '',
      href: sheetHref(one.id, typed),
      initial: initialOf(name),
      tint: tintOf(one.id),
      flaws: flawsCell(one.flaws),
      installed: isText(one.installed) ? one.installed : here.get(one.id) ?? null,
    };
  }).sort(byName);
}

/** What a catalog's reason says when the network is why: the Updates page is where a connection is looked at. */
const NETWORK = /network|connect|resolve|dns|timed? ?out|unreachable|offline|route/i;

/**
 * A banner for each catalog that could not be read, `{catalog, words,
 * network}`: `network` is true when the reason is the network, and the
 * banner then links to the Updates page. "The list shows the others." is
 * said only when the list has an app.
 */
export function unreadOf(answer) {
  const others = listOf(answer?.bundles).length > 0;
  return listOf(answer?.unread).filter((one) => isText(one?.catalog)).map((one) => {
    const why = isText(one.why) ? one.why.replace(/[.\s]+$/, '') : 'no reason was given';
    return {
      catalog: one.catalog,
      words: `The catalog ${one.catalog} could not be read: ${why}.${others ? ' The list shows the others.' : ''}`,
      network: NETWORK.test(why),
    };
  });
}

/**
 * What the table says when it has no row, as `{kind, words}`: no catalog is
 * on (no word was typed and nothing is listed, nothing unread), nothing
 * matches, or no catalog could be read. Null when there are rows.
 */
export function emptyOf(answer, words) {
  if (listOf(answer?.bundles).length > 0) return null;
  if (listOf(answer?.unread).length > 0) return { kind: 'unread', words: 'No app is listed: no catalog could be read.' };
  if (words.length === 0) return { kind: 'none', words: 'No catalog is turned on.', command: 'bundle catalog' };
  return { kind: 'nomatch', words: `No app in the catalogs matches “${words.join(' ')}”.` };
}

// ---- the ready-made setups ------------------------------------------------------

const NGINX = { id: NGINX_ID, name: 'nginx' };
const PHP = { id: PHP_ID, name: 'PHP-FPM' };
const POSTGRESQL = { id: POSTGRESQL_ID, name: 'PostgreSQL' };

/** The setups the machine does, each a link to the page that does it, with the apps it uses. */
const SETUPS = Object.freeze([
  {
    id: 'static', title: 'Static website', words: 'Pages and files from a folder, with HTTPS.', uses: [NGINX], href: '/websites/add?kind=static',
  },
  {
    id: 'php', title: 'PHP website with a database', words: 'For your own PHP code or a PHP app, with HTTPS.', uses: [NGINX, PHP, POSTGRESQL], href: '/websites/add?kind=php&database=1',
  },
  {
    id: 'database', title: 'A database', words: 'A database and its role for your own apps, with a password shown once.', uses: [POSTGRESQL], href: '/databases/add',
  },
  {
    id: 'private', title: 'Private access for your devices', words: 'Reach this server from your laptop and phone, from anywhere.', uses: [{ id: null, name: 'WireGuard' }], href: '/security#private',
  },
]);

/** "nginx", "nginx and PostgreSQL", "nginx, PHP-FPM and PostgreSQL". */
const listWords = (names) => (names.length < 2 ? names.join('') : `${names.slice(0, -1).join(', ')} and ${names.at(-1)}`);

/**
 * The setups that `text` matches (each word of it, in a title or a name of
 * what it uses), with what each needs installed first: `{id, title, words,
 * href, uses: [name], installs: [name], says}`. `says` is "Installs nginx
 * first" when an app is missing, else null. `installedIds` is the IDs of the
 * apps that are installed.
 */
export function setupsOf(text, installedIds) {
  const have = new Set(installedIds);
  const words = String(text ?? '').toLowerCase().split(/\s+/).filter(Boolean);
  return SETUPS.filter((setup) => {
    const hay = [setup.title, ...setup.uses.map((one) => one.name)].join(' ').toLowerCase();
    return words.every((word) => hay.includes(word));
  }).map((setup) => {
    const missing = setup.uses.filter((one) => one.id !== null && !have.has(one.id)).map((one) => one.name);
    return {
      id: setup.id,
      title: setup.title,
      words: setup.words,
      href: setup.href,
      uses: setup.uses.map((one) => one.name),
      installs: missing,
      says: missing.length > 0 ? `Installs ${listWords(missing)} first` : null,
    };
  });
}

// ---- the sheet ------------------------------------------------------------------

/** What the title line says of a key that this computer does not trust yet. */
export const UNKNOWN_KEY = 'signed by a key this computer does not know yet';

/** What the sheet says of the commands that run with the access of whoever runs them, and of those that do not. */
export const YOUR_ACCESS_WORDS = 'These run with the access of whoever runs them; the first time each asks, with a warning, in that person\'s terminal.';
export const SANDBOX_WORDS = 'Its commands run in its sandbox.';

/** What it says of installing an app that runs services. */
export const EVERYONE_WORDS = 'A bundle that runs services is installed for everyone.';

/** What it says of the folders of its services. */
export const DATA_WORDS = 'Made at its first start; only its account may read it.';

/** Whether a permission is one that the system asks for with a warning. */
const warns = (permission) => /warning/i.test(String(permission.tier ?? ''));

/** The services of the sheet, each as "nginx · as its own account, nginx · off until you turn it on". */
function servicesOfSheet(sheet) {
  const list = listOf(sheet.service_list).filter((one) => isText(one?.name)).map((one) => ({
    name: one.name,
    account: isText(one.account) ? one.account : one.name,
    boot: one.start === 'on' ? 'starts at boot' : 'off until you turn it on',
  }));
  if (Array.isArray(sheet.service_list)) {
    return list.map((one) => ({ ...one, words: `${one.name} · as its own account, ${one.account} · ${one.boot}` }));
  }
  return listOf(sheet.services).filter(isText).map((line) => ({ name: null, account: null, boot: null, words: line }));
}

/** The folders of the sheet's services: each service's, as the sheet says them, else as an app's data folders are worked out. */
const foldersOf = (sheet, services) => {
  if (Array.isArray(sheet.data)) return sheet.data.filter(isText);
  const named = services.filter((one) => one.name !== null);
  return named.length > 0 ? dataFolders(sheet, named) : [];
};

/** Whether a link of what the sheet uses is offered by an installed app `one`, as bundled matches it: by interface when it names one, else by socket. */
const offeredBy = (use, one) => listOf(one?.provides).some((link) => (isText(use.interface)
  ? link?.interface === use.interface
  : isText(use.socket) && link?.socket === use.socket));

/** The app that a well-known interface comes from, in "Needs PHP-FPM, which is not installed". */
const NEEDED = Object.freeze({ fastcgi: 'PHP-FPM' });

/**
 * The sheet of an app that is not installed yet, from `apps.sheet`'s answer
 * (`bundle sheet --json`), as the sections the page draws in order. `context`
 * is `{admin, backups, installed}`: `backups` is `GET /api/1/backups` (or null
 * when it could not be read), `installed` the installed apps as `{id, name,
 * provides}` (or null when they were not read, which says nothing of what the
 * sheet needs).
 */
export function sheetOf(sheet, { admin = false, backups = null, installed = null } = {}) {
  const trust = sheet.trust ?? {};
  const source = isText(sheet.catalog) ? `${capital(sheet.catalog)} catalog` : null;
  const unknown = trust.kind === 'unknown';
  const services = servicesOfSheet(sheet);
  const flaws = flawsOf(sheet, null);
  const worst = worstOf(flaws.map((flaw) => flaw.severity));
  const folders = foldersOf(sheet, services);
  const covered = backups === null ? null : listOf(backups.places).some((place) => listOf(place?.places).includes('/shared'));
  const mine = listOf(sheet.uses).map((use) => {
    const by = installed === null ? null : installed.filter((one) => offeredBy(use, one)).map((one) => one.name ?? one.id);
    return { words: linkWords(use), by, missing: by !== null && by.length === 0, name: NEEDED[use.interface] ?? null };
  }).filter((use) => isText(use.words));
  const refusal = isText(sheet.refusal) ? sheet.refusal : null;
  const already = isText(sheet.installed) ? sheet.installed : null;
  const sha256 = /^[0-9a-f]{64}$/.test(sheet.sha256 ?? '') ? sheet.sha256 : null;
  const commands = listOf(sheet.commands_with_your_access).filter(isText);
  const download = Number.isFinite(sheet.download_bytes) ? `${bytesWords(sheet.download_bytes)} to download` : null;
  const size = Number.isFinite(sheet.bytes) ? `${bytesWords(sheet.bytes)} installed` : null;
  return {
    id: sheet.id,
    name: isText(sheet.name) ? sheet.name : sheet.id,
    version: isText(sheet.version) ? sheet.version : '',
    initial: initialOf(isText(sheet.name) ? sheet.name : sheet.id),
    tint: tintOf(sheet.id),
    summary: isText(sheet.summary) ? sheet.summary : null,
    line: { words: [source, unknown ? UNKNOWN_KEY : signedWords(trust), isText(sheet.license) ? sheet.license : null].filter(Boolean).join(' · '), warn: unknown },
    publisher: isText(sheet.publisher) ? sheet.publisher : null,
    support: isText(sheet.support) ? { words: sheet.support, link: /^https?:\/\/\S+$/.test(sheet.support) } : null,
    size: [download, size].filter(Boolean).join(', ') || null,
    flaws: {
      worst,
      urgent: urgent(worst),
      lines: flaws.map((flaw) => ({ words: flawLine(flaw), fixed: fixedWords(flaw) })),
      none: noFlawsWords(sheet),
    },
    services,
    everyone: sheet.services_need_everyone === true ? EVERYONE_WORDS : null,
    may: listOf(sheet.permissions).filter(isPermission).map((one) => ({
      words: `${permissionLine(one)}${one.need === 'optional' ? ' (optional)' : ''}`,
      warning: warns(one),
      why: isText(one.why) ? one.why : null,
    })),
    commands: { names: commands, words: commands.length > 0 ? YOUR_ACCESS_WORDS : SANDBOX_WORDS },
    data: folders.length === 0 ? null : {
      folders,
      words: DATA_WORDS,
      backup: covered === null ? null : covered
        ? { words: 'Snapshots and backups copy /shared.', link: null }
        : { words: 'No backup place copies /shared yet.', link: { words: 'Add a backup place', href: '/snapshots/backups' } },
    },
    provides: listOf(sheet.provides).map(linkWords).filter(isText),
    uses: mine,
    needs: mine.filter((use) => use.missing).map((use) => (use.name ? `Needs ${use.name}, which is not installed.` : `Needs ${use.words}, which no installed app provides.`)),
    installed: already === null ? null : { words: `Version ${already} is installed. Updating is on its page.`, href: appHref(sheet.id) },
    refusal,
    sha256,
    install: { can: admin && refusal === null && already === null && sha256 !== null, reader: !admin, command: installCommand(sheet.id) },
  };
}

/** The installed apps as the sheet's `uses` is matched against: `{id, name, provides}` from the `bundle info` of each. */
export const providersOf = (infos) => infos.filter((one) => isText(one?.id)).map((one) => ({
  id: one.id, name: isText(one.name) ? one.name : one.id, provides: listOf(one.provides),
}));

/** What the page says when the sheet could not be had: why, and that nothing was kept. */
export const fetchWords = (error) => `The app could not be fetched: ${([error?.detail, error?.title].find(isText) ?? 'no reason was given').replace(/[.\s]+$/, '')}.`;

/** What the page says when the catalog offers another file than the sheet that was read. */
export const STALE_WORDS = 'The catalog has a newer version than the sheet you read. Read it again.';

/** The ID of the refusal that says the catalog's file is not the one whose sheet was read. */
export const STALE_ID = 'APP-102';

/** Whether the install failed because the catalog moved on: the error says so, or the task's note or log does. */
export const isStale = (error, task = null) => error?.id === STALE_ID
  || [task?.note, ...listOf(task?.log)].some((line) => typeof line === 'string' && line.includes(STALE_ID));

// ---- after the install --------------------------------------------------------------

/** What comes next after an app, by its ID: nginx's websites, PostgreSQL's databases, PHP's pages; any other app has its page. */
const NEXT = new Map([
  [NGINX_ID, { words: 'Add a website', href: '/websites/add' }],
  [POSTGRESQL_ID, { words: 'Add a database', href: '/databases/add' }],
  [PHP_ID, { words: 'Add a PHP website', href: '/websites/add?kind=php' }],
]);

/** The next step after the install of `id`: `{words, href}`. */
export const nextStep = (id) => NEXT.get(id) ?? { words: 'Open its page', href: appHref(id) };

/**
 * What stands after the install, from `bundle info` of the new app (`info`)
 * and Spark's list of services (`show`, or null): `{installed, services,
 * data, next}`. Each service is `{name, href, on, words, kind}`, with the
 * words of its state when it is on and "off" when it is not; `kind` is the
 * state's colour.
 */
export function afterOf(info, show = null) {
  const services = servicesOf(info, show) ?? [];
  return {
    installed: `Installed ${info.name} ${info.version} for everyone.`,
    services: services.map((one) => {
      const on = one.boot !== 'off at boot';
      const state = one.state ? `${one.state.word.toLowerCase()}, ` : '';
      return {
        name: one.name,
        href: one.href,
        on,
        words: on ? `Its service ${one.name} is on: ${state}${one.boot}.` : `Its service ${one.name} is off.`,
        kind: one.state?.kind ?? 'off',
      };
    }),
    data: services.length > 0 ? `Its data is in ${dataFolders(info, services).join(', ')} once it has started.` : null,
    next: nextStep(info.id),
  };
}
