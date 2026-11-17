// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The list of websites: a line for nginx or PHP-FPM when it does not run, and
// a table of each site with what it serves, how big it is, its visits of
// yesterday, its HTTPS and its state, which becomes a card for each on a
// phone. Anyone reads it; an administrator adds a site and asks nginx to check
// them as they are.

import { fill, h, icon } from '../../dom.js';
import {
  httpsOf, kindWords, READER_WORDS, serverLines, sizeWords, stateOf, visitsWords,
} from '../../lib/website-model.js';
import { hrefOf } from '../../lib/website-route.js';
import { copyBox } from '../snapshots/copy-box.js';
import { makeCheck } from './check.js';
import {
  dateOf, hasPage, readWebsites,
} from './read.js';

/** The columns of the table, with the class of those that hold a number. */
const COLUMNS = [['Site'], ['Serves'], ['Size', 'num'], ['Visits yesterday', 'num'], ['HTTPS'], ['State']];

/** The command a reader types, in a box that copies it. */
const ADD_COMMAND = 'elevate website add NAME';

/** The line above the list for a server that does not run: its words, and the one thing to do about it. */
function serverLine(view, admin, line, changed) {
  const act = [];
  if (line.enable && admin) {
    const button = h('button', { class: 'btn default', type: 'button', 'data-action': `turn-on-${line.key}` }, 'Turn it on');
    button.addEventListener('click', async () => {
      const result = await view.api.run('service.enable', { name: line.enable }, {
        button,
        words: { unlock: `turn ${line.key === 'nginx' ? 'nginx' : 'PHP-FPM'} on`, running: `Turning ${line.key === 'nginx' ? 'nginx' : 'PHP-FPM'} on`, done: `${line.key === 'nginx' ? 'nginx' : 'PHP-FPM'} is on.` },
      });
      if (result.ok) changed();
    });
    act.push(button, view.api.inline('service.enable', { name: line.enable }));
  }
  if (line.sheet && hasPage(line.sheet)) act.push(h('a', { class: 'btn default', href: line.sheet }, 'Add it from the apps'));
  return h('div', { class: 'banner warn', 'data-line': line.key }, icon('warn'), h('div', {}, h('p', {}, line.words), act.length > 0 && h('div', { class: 'btns' }, act)));
}

export async function showList(view) {
  const admin = view.session.admin === true;
  const errors = h('div', { class: 'web-errors' });
  const body = h('div', { class: 'web-list' });
  const check = admin ? makeCheck(view, { errors, onResult: () => draw() }) : null;
  view.head({
    title: 'Websites',
    crumbs: [{ title: 'Apps', path: '/apps' }],
    commands: true,
    buttons: admin ? [h('a', { class: 'btn default', href: '/websites/add' }, icon('plus'), 'Add a website'), check.button] : [],
  });
  view.main.append(h('div', { class: 'web-page' }, !admin && h('p', { class: 'muted' }, READER_WORDS), errors, body));

  let data = null;

  /** The state of `site`: what nginx's check found of it when it was asked, else what the list says. */
  function stateOfSite(site, read) {
    const found = check?.problems()[site.name]?.[0];
    return found ? { tone: 'bad', words: found.words, id: found.id ?? null } : stateOf(site, { nginx: read.nginx, phpFpm: read.phpFpm });
  }

  function rowOf(site, read) {
    const https = httpsOf(site, data.certificates, dateOf(view));
    const state = stateOfSite(site, read);
    const [first, ...others] = site.names?.length > 0 ? site.names : [site.name];
    return h('tr', { 'data-name': site.name },
      h('td', { 'data-label': 'Site' },
        h('a', { href: hrefOf(site.name) }, h('b', {}, first)),
        others.length > 0 && h('small', {}, others.join(', ')),
        (site.by_hand || site.added_by_hand) && h('span', { class: 'pill warn', title: 'Kept as you wrote it' }, 'Changed by hand')),
      h('td', { 'data-label': 'Serves' }, kindWords(site), site.folder && h('small', { class: 'mono' }, site.folder)),
      h('td', { 'data-label': 'Size', class: 'num' }, sizeWords(site.size)),
      h('td', { 'data-label': 'Visits yesterday', class: 'num' }, visitsWords(site.visits)),
      h('td', { 'data-label': 'HTTPS', class: https.tone === 'warn' ? 'web-tone-warn' : null }, https.words),
      h('td', { 'data-label': 'State' },
        h('span', { class: `st ${state.tone}` }, state.words),
        state.id && h('small', { class: 'mono' }, state.id)));
  }

  const again = h('button', { class: 'btn small', type: 'button', 'data-action': 'read-again' }, 'Read again');
  again.addEventListener('click', () => load());

  function draw() {
    if (data === null) {
      fill(body, h('p', { class: 'muted' }, 'Reading the websites…'));
      return;
    }
    const { read } = data;
    if (read === null) {
      fill(body, h('p', { class: 'muted' }, 'The websites could not be read.'), h('div', { class: 'btns' }, again));
      return;
    }
    fill(body,
      serverLines(read.answer, read.sites).map((line) => serverLine(view, admin, line, load)),
      check?.element,
      read.sites.length === 0
        ? h('div', { class: 'web-empty' },
          h('p', {}, h('b', {}, 'No websites yet.')),
          admin
            ? h('a', { class: 'btn default', href: '/websites/add' }, icon('plus'), 'Add a website')
            : [h('p', { class: 'small muted' }, 'An administrator adds one. At a terminal:'), copyBox(ADD_COMMAND, 'The command that adds a website')])
        : h('div', { class: 'tbl-box' },
          h('table', { class: 'tbl stack' },
            h('caption', { class: 'sr-only' }, 'Websites: each with what it serves, its size, its visits yesterday, its HTTPS and its state'),
            h('thead', {}, h('tr', {}, COLUMNS.map(([title, kind]) => h('th', { scope: 'col', class: kind }, title)))),
            h('tbody', {}, read.sites.map((site) => rowOf(site, read))))),
      h('div', { class: 'web-foot' }, again, view.api.inline('websites.list', {})));
  }

  async function load() {
    fill(errors);
    const got = await readWebsites(view, errors);
    if (got === null) return;
    data = got;
    draw();
  }

  draw();
  await load();
}
