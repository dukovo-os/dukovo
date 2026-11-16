// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// An app's page, the panel beside the list: what it is and where it came
// from, the update or the flaws that wait, its services, what it may use and
// what uses it, its data, its versions, and the buttons that update, hold,
// go back and remove it. A reader sees every fact and no button. What is
// drawn changes only when what it says does, and a button in hand is found
// again after a redraw, so that the keyboard stays where it was.

import { fill, h, icon } from '../../dom.js';
import { copyBox } from '../snapshots/copy-box.js';
import {
  ASKED_WORDS, bannerOf, byline, dataFolders, fixedWords, flawLine, heldWords, linksOf, manageOf, mayUseOf, noFlawsWords,
  READER_LINE, RISK_WORDS, ruleWords, runsAs, serviceLines, servicesOf, sizeWords, usedByOf,
} from './app-model.js';
import { ownCommands } from './list-model.js';
import { onPress } from './list.js';
import { square } from './mark.js';
import { jobWords, updateCall } from './run-model.js';
import { versionsTable } from './versions.js';

const capital = (words) => words.charAt(0).toUpperCase() + words.slice(1);

/** A row of the facts: its name and what it holds, none when there is nothing to hold. */
const fact = (name, ...content) => (content.flat().filter(Boolean).length === 0 ? null : [h('dt', {}, name), h('dd', {}, content)]);

/** Lines, one under another. */
const lines = (items) => h('ul', { class: 'apps-lines' }, items.map((item) => h('li', {}, item)));

/**
 * The panel. `act` is what the buttons do: `update(button, {yes})`,
 * `hold(button, held)`, `back(button)` and `remove(button)`. Gives
 * `{element, set, paint, focus}`: `set(data)` draws the app of `data.info`
 * (`bundle info --json`) with its list row `data.row`, Spark's list of
 * services `data.show` (or null), `updates.settings` as `data.settings` (or
 * null), the list of what the app manages, `data.managed` (the websites of
 * nginx or the databases of PostgreSQL, or null), the sheet of an update that stopped to ask, `data.sheet` (or null),
 * `data.admin`, and `data.links` (`{updates, services}`: which pages there
 * are); `paint(jobs)` puts the words of a running task in the page and keeps
 * the buttons off while it runs; `focus(action)` puts focus on the button
 * that does `action`.
 */
export function makeApp(view, act) {
  const element = h('section', { class: 'card apps-panel' });
  let drawn = null;
  let current = null;
  let jobs = null;

  /** The banner over the facts: the update that waits and the flaws of the version installed. */
  function bannerNode(banner) {
    return h('div', { class: `banner ${banner.kind}`, role: 'note' }, icon(banner.kind === 'info' ? 'info' : 'warn'),
      h('div', {},
        h('p', {}, h('b', {}, banner.heading)),
        banner.flaws.length > 0 && h('ul', { class: 'apps-lines' }, banner.flaws.map((flaw) => h('li', {}, flawLine(flaw), ' ', h('span', { class: 'muted' }, fixedWords(flaw))))),
        banner.flaws.length > 0 && h('p', { class: 'small' }, RISK_WORDS)));
  }

  /** The banner of an update that stopped to ask: the sheet it came with, and Update anyway. */
  function askedNode(info, sheet, admin) {
    const go = admin ? h('button', { class: 'btn', type: 'button', 'data-action': 'update-yes', 'data-guard': '' }, 'Update anyway') : null;
    if (go) onPress(go, () => act.update(go, { yes: true }));
    return h('div', { class: 'banner warn', role: 'note' }, icon('warn'),
      h('div', {},
        h('p', {}, h('b', {}, ASKED_WORDS)),
        sheet !== '' && h('pre', { class: 'cmdtext apps-sheet', tabindex: '0', 'aria-label': `What the update of ${info.name} asks for` }, sheet),
        go && h('div', { class: 'apps-action' }, go, view.api.inline(updateCall(info.id, { yes: true }).action, updateCall(info.id, { yes: true }).args))));
  }

  /** The rows of facts, from `bundle info`. */
  function facts({ info, show, settings, managed, links }) {
    const services = servicesOf(info, show);
    const used = usedByOf(info);
    const provides = linksOf(info, 'provides');
    const uses = linksOf(info, 'uses');
    const may = mayUseOf(info);
    const folders = dataFolders(info, services ?? []);
    const size = sizeWords(info);
    const rule = ruleWords(settings);
    const noFlaws = noFlawsWords(info);
    const manage = manageOf(info.id, managed, links);
    return h('dl', { class: 'rows' },
      fact('Service', services === null
        ? lines(serviceLines(info))
        : services.map((one) => h('div', {}, h('a', { href: one.href }, one.name), ' · ',
          one.state ? [h('span', { class: `st ${one.state.kind}` }, one.state.word), `, ${one.boot}`] : capital(one.boot)))),
      services && services.length > 0 && fact('Runs as', runsAs(services)),
      manage && fact(manage.name, manage.count === null ? null : `${manage.count} · `, h('a', { href: manage.href }, 'Manage')),
      fact('May use', lines([...may, may.length > 0 ? 'nothing else' : 'nothing'])),
      fact('Provides', provides && lines(provides)),
      fact('Uses', uses && lines(uses)),
      used !== null && fact('Used by', used.length === 0
        ? 'Nothing installed uses it.'
        : lines(used.map((one) => [h('a', { href: one.href }, one.name), one.what && ` (${one.what})`]))),
      fact('Data', folders.length > 0
        ? [lines(folders.map((folder) => h('code', {}, folder))),
          h('small', { class: 'muted' }, 'A bundle keeps its settings in its data folder; edits made by hand stay through updates.')]
        : (typeof info.data === 'string' ? info.data : null)),
      fact('Flaws', noFlaws),
      fact('Size', size),
      fact('Updates', rule && [rule, links.updates && [' · ', h('a', { href: '/updates' }, 'Change on the Updates page')]]));
  }

  /** The buttons under the versions: each with the command it runs under it. */
  function buttons({ info, row, admin, services }) {
    if (info.for_everyone === false) {
      return h('div', { class: 'apps-own' },
        h('p', { class: 'small muted' }, 'This app is yours, not everyone\'s. No button here runs it: use these commands.'),
        copyBox(ownCommands(info.id).update, `Command to update ${info.name}`),
        copyBox(ownCommands(info.id).remove, `Command to remove ${info.name}`));
    }
    if (!admin) return h('p', { class: 'small muted' }, READER_LINE);
    const make = (action, label, click, command, { kind = '', href = null } = {}) => {
      if (href) return h('a', { class: 'btn', href, 'data-action': action }, label);
      const button = h('button', { class: `btn${kind ? ` ${kind}` : ''}`, type: 'button', 'data-action': action, 'data-guard': '' }, label);
      onPress(button, click);
      return h('div', { class: 'apps-action' }, button, command && view.api.inline(command.action, command.args));
    };
    const held = info.held === true;
    return h('div', { class: 'apps-buttons' },
      held && h('p', { class: 'small muted' }, heldWords(info)),
      h('div', { class: 'btns' },
        row.update.state === 'update' && make('update', `Update to ${row.update.to}`, (button) => act.update(button, {}), updateCall(info.id), { kind: 'default' }),
        make('hold', held ? 'Release the hold' : `Hold at ${info.version}`, (button) => act.hold(button, held), { action: held ? 'apps.unhold' : 'apps.hold', args: { id: info.id } }),
        services && services.length > 0 && make('settings', 'Settings…', null, null, { href: services[0].href }),
        make('remove', 'Remove…', (button) => act.remove(button), { action: 'apps.remove', args: { id: info.id, data: 'keep' } }, { kind: 'danger' })));
  }

  /** The words of a running task, and the guard on the buttons. */
  function paint(next = jobs) {
    jobs = next;
    if (current === null) return;
    const job = jobs?.of(current.id, current.updates) ?? null;
    const line = element.querySelector('.apps-job');
    const words = job ? jobWords(job) : '';
    if (line.textContent !== words) line.textContent = words;
    line.hidden = job === null;
    for (const button of element.querySelectorAll('[data-guard]')) {
      if (job) button.setAttribute('aria-disabled', 'true');
      else button.removeAttribute('aria-disabled');
    }
  }

  /** Draws the app, unless what it says is what is drawn. */
  function set(data) {
    const {
      info, row, show, settings, managed, sheet, admin, links,
    } = data;
    const key = JSON.stringify([info, row.update, row.flaws, show, settings, managed, sheet, admin, links]);
    if (key === drawn) return;
    drawn = key;
    current = { id: info.id, updates: row.update.state === 'update' };
    // The key that had focus, found again by what it does: the button that held the keyboard may now say something else, or be gone.
    const here = document.activeElement;
    const had = here?.closest?.('.apps-panel') === element ? here.getAttribute('data-action') ?? '' : null;

    const banner = bannerOf(info, row.update);
    const services = servicesOf(info, show);
    const title = h('h2', { id: 'apps-h', tabindex: '-1' }, `${info.name} ${info.version}`);
    fill(element,
      h('div', { class: 'apps-title' }, square(row, true), h('div', {}, title, h('span', { class: 'muted' }, byline(info)))),
      h('p', { class: 'apps-job', role: 'status', hidden: true }),
      sheet !== null && askedNode(info, sheet, admin),
      banner && bannerNode(banner),
      facts({ info, show, settings, managed, links }),
      h('h3', {}, 'Versions'),
      versionsTable(view, {
        info, update: row.update, admin: admin && info.for_everyone !== false, back: (button) => act.back(button),
      }),
      buttons({
        info, row, admin, services,
      }));
    element.setAttribute('aria-labelledby', 'apps-h');
    paint();
    if (had !== null) focus(had);
  }

  /** Puts focus on the button that does `action`, or on the heading when it is gone. */
  function focus(action) {
    (element.querySelector(`[data-action="${action}"]`) ?? element.querySelector('#apps-h'))?.focus();
  }

  return {
    element, set, paint, focus,
  };
}
