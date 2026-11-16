// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the sheet of an app that is not installed yet says, and what stands
// after its install, as plain values so that a test reaches every branch: its
// trust, flaws, services, permissions with the warning ones marked, commands,
// data and what it uses, the words of an install that the catalog moved past,
// and the next step for nginx, PostgreSQL, PHP and any other app. The words for
// flaws, permissions, services and links are those of the Apps pages
// (pages/apps/app-model.js); this file adds what only a sheet says.

import { appHref, capital, isText, listOf, urgent, worstOf } from '../pages/apps/apps-model.js';
import {
  dataFolders, fixedWords, flawLine, flawsOf, isPermission, linkWords, noFlawsWords, permissionLine, servicesOf, signedWords,
} from '../pages/apps/app-model.js';
import { initialOf, tintOf } from '../pages/apps/mark.js';
import { NGINX_ID, PHP_ID, POSTGRESQL_ID } from './known-bundles.js';
import { bytesWords } from './size-words.js';

/** The command that installs `id` for everyone, which asks its question at the terminal. */
export const installCommand = (id) => `elevate bundle install ${id} --for-everyone`;

/** What a reader is told in place of Install. */
export const READER_WORDS = 'An administrator installs apps for everyone.';

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
