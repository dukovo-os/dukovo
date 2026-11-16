// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The sheet of an app that is not installed yet, beside the list on a wide
// screen and alone on a phone: who made it and whether it is signed, what it
// is, what it may use, its services and its data, drawn in the order that the
// plain sheet of `bundle install` has. Install for everyone is the sheet's
// answer: it runs as a task, pinned by the sheet's SHA-256, and what stands
// after it is a list of what happened, with the service to turn on and the
// next step. The panel reads the sheet when it is made and is drawn again only
// when what it says changes.

import { fill, h, icon } from '../../dom.js';
import { addHref, sheetPath } from '../../lib/catalog-model.js';
import { helpPath } from '../../lib/problem-model.js';
import {
  afterOf, fetchWords, providersOf, READER_WORDS, sheetOf, STALE_WORDS,
} from '../../lib/sheet-model.js';
import { askToUnlock, isUnlocked } from '../../unlock.js';
import { RISK_WORDS } from '../apps/app-model.js';
import { square } from '../apps/mark.js';
import { copyBox } from '../snapshots/copy-box.js';
import {
  installWords, readServices, runInstall, soft, turnOn,
} from './install.js';

/** A section of the sheet: a heading and what is under it; none when there is nothing under it. */
const section = (title, ...content) => (content.flat(Infinity).filter(Boolean).length === 0 ? null : h('section', { class: 'add-sec' }, h('h3', {}, title), content));

/** Lines, one under another. */
const lines = (items) => h('ul', { class: 'add-lines' }, items.map((item) => h('li', {}, item)));

/**
 * The panel of the app `id`. `typed` is what the search holds, kept in the
 * address of the list the panel leads back to. `admin` says who looks;
 * `apps()` gives `GET /api/1/apps` as the page has it (or null); `installed()`
 * is called when an install is done, so that the list says so. Gives
 * `{element, load}`: `load()` reads the sheet and draws it.
 */
export function makePanel(view, {
  id, typed, admin, apps, installed,
}) {
  const element = h('section', { class: 'card add-panel', 'aria-labelledby': 'add-h' });
  let latest = 0;
  let sheet = null;
  let running = false;

  /** The heading, which takes focus when the panel is made new. */
  const heading = (words) => h('h2', { id: 'add-h', tabindex: '-1' }, words);

  /** The buttons that leave the sheet: back to the list, with what was typed kept. */
  const backLink = (words) => h('a', { class: 'btn', href: addHref(typed), 'data-action': 'close' }, words);

  // ---- reading ----

  /** The installed apps, each as `bundle info` says it, for the sheet to find what it uses; null when they were not read. */
  async function readInstalled() {
    const list = apps()?.bundles;
    if (!Array.isArray(list)) return null;
    const infos = await Promise.all(list.filter((one) => one?.for_everyone !== false && one?.id)
      .map((one) => soft(view.api.get(`/api/1/apps/${encodeURIComponent(one.id)}`))));
    return providersOf(infos.filter(Boolean));
  }

  /** Reads the sheet, which downloads the app, and draws it. Only the last read counts. */
  async function load() {
    latest += 1;
    const mine = latest;
    fill(element, heading(id), h('p', { class: 'muted', role: 'status' }, 'Reading the sheet. The app is downloaded to read it, so this can take a moment.'));
    try {
      const [read, backups] = await Promise.all([view.api.get(sheetPath(id)), soft(view.api.get('/api/1/backups'))]);
      if (mine !== latest) return;
      const have = Array.isArray(read?.uses) && read.uses.length > 0 ? await readInstalled() : null;
      if (mine !== latest) return;
      sheet = sheetOf(read, { admin, backups, installed: have });
      drawSheet();
    } catch (error) {
      if (mine !== latest || error?.name === 'AbortError') return;
      drawFailure(error);
    }
  }

  /** The sheet could not be had: why, that nothing was kept, and a way to try again. */
  function drawFailure(error) {
    const again = h('button', { class: 'btn', type: 'button', 'data-action': 'again' }, 'Try again');
    again.addEventListener('click', load);
    fill(element, heading(id),
      h('div', { class: 'banner bad', role: 'alert' }, icon('warn'),
        h('div', {}, h('p', {}, fetchWords(error)), h('p', { class: 'small' }, 'Nothing was installed or kept.'),
          error?.id && h('p', { class: 'small' }, h('a', { href: helpPath(error.id) }, `What ${error.id} means`)))),
      h('div', { class: 'btns' }, again, backLink('Cancel')));
    element.querySelector('#add-h').focus();
  }

  // ---- the sheet ----

  /** The permission lines of "May use": a warning one starts with the word, and says why. */
  const mayLines = (may) => [
    ...may.map((one) => [one.warning && h('b', { class: 'warn' }, 'Warning: '), one.words, one.why && h('small', {}, `In its words: “${one.why}”`)]),
    `${may.length > 0 ? 'and ' : ''}nothing else`,
  ];

  /** "fastcgi at /run/php-fpm.sock", with the installed apps that offer it. */
  const useLine = (use) => (use.by === null || use.by.length === 0 ? use.words : `${use.words} (offered by ${use.by.join(', ')})`);

  /** The sections of the sheet. */
  function sections(model) {
    return [
      section('Who made it', model.publisher, model.support && (model.support.link
        ? h('a', { href: model.support.words, rel: 'noopener noreferrer' }, 'Support')
        : `Support: ${model.support.words}`)),
      section('Its size', model.size),
      section('Flaws', model.flaws.none, model.flaws.lines.length > 0 && [
        h('ul', { class: 'add-lines' }, model.flaws.lines.map((one) => h('li', {}, one.words, ' ', h('span', { class: 'muted' }, one.fixed)))),
        h('p', { class: 'small' }, RISK_WORDS),
      ]),
      section('Runs as services', model.services.length > 0 && lines(model.services.map((one) => one.words)), model.everyone && h('p', { class: 'small' }, model.everyone)),
      section('May use', lines(mayLines(model.may))),
      section('Commands that run with your access',
        model.commands.names.length > 0 && lines(model.commands.names.map((name) => h('code', {}, name))), h('p', { class: 'small' }, model.commands.words)),
      model.data && section('Its data', lines(model.data.folders.map((folder) => h('code', {}, folder))), h('p', { class: 'small' }, model.data.words),
        model.data.backup && h('p', { class: 'small' }, model.data.backup.words, model.data.backup.link && [' ', h('a', { href: model.data.backup.link.href }, model.data.backup.link.words)])),
      section('Works with', model.provides.length > 0 && h('p', {}, 'Provides ', model.provides.join(', ')), model.uses.length > 0 && h('p', {}, 'Uses ', model.uses.map(useLine).join(', ')),
        model.needs.length > 0 && lines(model.needs.map((words) => h('span', { class: 'warn' }, words)))),
    ];
  }

  /** The buttons at the end of the sheet. */
  function buttons(model) {
    const box = h('div', { class: 'add-command', hidden: true }, copyBox(model.install.command, `Command to install ${model.name} for everyone`));
    const show = h('button', { class: 'btn', type: 'button', 'data-action': 'show-command', 'aria-expanded': 'false' }, 'Show command');
    show.addEventListener('click', () => {
      box.hidden = !box.hidden;
      show.setAttribute('aria-expanded', String(!box.hidden));
    });
    const install = model.install.can ? h('button', { class: 'btn default', type: 'button', 'data-action': 'install', 'data-guard': '' }, 'Install for everyone') : null;
    install?.addEventListener('click', () => {
      if (install.getAttribute('aria-disabled') !== 'true') start();
    });
    return [
      model.install.reader && model.refusal === null && model.installed === null && h('p', { class: 'small muted' }, READER_WORDS),
      h('div', { class: 'btns add-buttons' }, install, show, backLink('Cancel')),
      box,
    ];
  }

  /** Draws the sheet. */
  function drawSheet() {
    const model = sheet;
    const progress = h('div', { class: 'add-run', role: 'status' });
    fill(element,
      h('div', { class: 'add-title' }, square(model, 'add-ic', true), h('div', {}, heading(`${model.name} ${model.version}`),
        h('span', { class: model.line.warn ? 'warn' : 'muted' }, model.line.warn && icon('warn'), model.line.words))),
      model.summary && h('p', {}, model.summary),
      model.refusal && h('div', { class: 'banner bad', role: 'note' }, icon('warn'), h('div', {}, h('p', {}, h('b', {}, 'This app would not be installed. '), model.refusal))),
      model.installed && h('div', { class: 'banner info', role: 'note' }, icon('info'), h('div', {}, h('p', {}, model.installed.words, ' ', h('a', { href: model.installed.href }, 'Open its page')))),
      sections(model),
      progress,
      buttons(model));
  }

  // ---- install ----

  /** Puts the buttons that start another thing off while an install runs. */
  const guard = (on) => {
    for (const node of element.querySelectorAll('[data-guard]')) {
      if (on) node.setAttribute('aria-disabled', 'true');
      else node.removeAttribute('aria-disabled');
    }
  };

  /** Install for everyone: unlock first when the console is locked, then run the step and say how it ended. */
  async function start() {
    if (running || !(isUnlocked() || await askToUnlock(view.session, installWords(sheet.name).unlock))) return;
    running = true;
    guard(true);
    const progress = element.querySelector('.add-run');
    fill(progress);
    try {
      const outcome = await runInstall(view, sheet, progress);
      if (outcome.left) return;
      if (outcome.ok) {
        installed();
        await drawAfter(outcome.info);
      } else {
        drawStopped(progress, outcome.stale);
      }
    } catch (error) {
      if (error?.name !== 'AbortError') view.problem(error);
    } finally {
      running = false;
      guard(false);
    }
  }

  /** What stands when the install did not end: Try again, and Read it again when the catalog moved on. */
  function drawStopped(progress, stale) {
    const again = h('button', { class: 'btn', type: 'button', 'data-action': 'try-again' }, 'Try again');
    again.addEventListener('click', start);
    const reread = stale ? h('button', { class: 'btn default', type: 'button', 'data-action': 'read-again' }, 'Read it again') : null;
    reread?.addEventListener('click', load);
    progress.append(...[
      stale && h('p', { class: 'warn', role: 'alert' }, STALE_WORDS),
      h('p', { class: 'small' }, 'Nothing was installed.'),
      h('div', { class: 'btns' }, reread, again),
    ].filter(Boolean));
  }

  // ---- after ----

  /** One line of what happened: the service, with Turn it on when it is off. */
  function serviceLine(one, info) {
    const on = h('button', { class: 'btn small', type: 'button', 'data-action': `turn-on-${one.name}` }, 'Turn it on');
    on.addEventListener('click', async () => {
      const done = await turnOn(view, on, one.name);
      if (done?.ok) drawAfter(info);
    });
    return h('li', {}, icon('check'), h('span', {}, h('span', { class: one.on ? `st ${one.kind}` : null }, one.words), ' ',
      !one.on && on, ' ', h('a', { href: one.href }, 'Open its service')));
  }

  /** What stands after the install, from the new app's `bundle info` and Spark's services. */
  async function drawAfter(info) {
    const after = afterOf(info, await readServices(view));
    fill(element,
      h('div', { class: 'add-title' }, square(sheet, 'add-ic', true), h('div', {}, heading(`${sheet.name} ${sheet.version}`), h('span', { class: 'muted' }, 'Installed'))),
      h('h3', {}, 'What happened'),
      h('ul', { class: 'add-done', 'data-installed': info.id },
        h('li', {}, icon('check'), after.installed),
        after.services.map((one) => serviceLine(one, info)),
        after.data && h('li', {}, icon('check'), after.data)),
      h('div', { class: 'btns' }, h('a', { class: 'btn default', href: after.next.href, 'data-action': 'next' }, after.next.words), backLink('Back to the catalogs')));
    element.querySelector('#add-h').focus();
  }

  return { element, load };
}
