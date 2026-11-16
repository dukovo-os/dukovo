// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the list and a database's page both draw from a read: the line about the
// server and what to do when it does not run, the error PostgreSQL found in its
// own files, and the notice that a database's indexes are out of date.

import { h, icon } from '../../dom.js';
import { copyBox } from '../snapshots/copy-box.js';
import { helpPath } from '../../lib/problem-model.js';
import { PAGES } from '../../pages.js';
import { matchPage } from '../../shell/route-model.js';
import {
  MISMATCH_WORDS, problemWords, reindexLines, serverLine, serverState, SERVICE, SHEET_PATH, STATE_WORDS,
} from './model.js';

/** Where PostgreSQL's service is. */
const SERVICE_PATH = `/services/${SERVICE}`;

/** Whether the console has a page at `path`, so that a link to it is not a dead end. */
export const hasPage = (path) => matchPage(PAGES, path)?.entry.load != null;

/** A banner of `kind` with `iconName`, the `words` and what else is in `more`. */
export const banner = (kind, iconName, words, ...more) => h('div', { class: `banner ${kind}` },
  icon(iconName), h('div', {}, h('p', {}, words), ...more));

/**
 * The line about the server, for a read: that it runs and where it listens,
 * or why it does not and the one thing to do about it. `changed()` is called
 * after Turn it on ran.
 */
export function serverCard(view, read, changed) {
  const state = serverState(read.server);
  if (state === 'on') {
    return h('p', { class: 'db-server' }, h('span', { class: 'pill ok' }, 'Running'), ' ', serverLine(read.server),
      hasPage(SERVICE_PATH) && [' · ', h('a', { href: SERVICE_PATH }, 'PostgreSQL\'s service')]);
  }
  const act = {
    missing: () => hasPage(SHEET_PATH) && h('a', { class: 'btn default', href: SHEET_PATH }, 'Add PostgreSQL'),
    off: () => {
      const button = h('button', { class: 'btn default', type: 'button', 'data-action': 'turn-on' }, 'Turn it on');
      button.addEventListener('click', async () => {
        const result = await view.api.run('service.enable', { name: SERVICE }, {
          button, words: { unlock: 'turn PostgreSQL on', running: 'Turning PostgreSQL on', done: 'PostgreSQL is on.' },
        });
        if (result.ok) changed();
      });
      return h('div', { class: 'btns' }, button, view.api.inline('service.enable', { name: SERVICE }));
    },
    stopped: () => hasPage(SERVICE_PATH) && h('a', { class: 'btn', href: SERVICE_PATH }, 'Open the service'),
  }[state]();
  return banner('warn', 'warn', STATE_WORDS[state], state === 'off' && h('p', { class: 'small muted' }, 'Its first start makes the cluster.'), act || null);
}

/** What PostgreSQL found wrong in its own files, as a banner for each, or null when it found nothing. */
export function problemsBanner(read) {
  if (read.problems.length === 0) return null;
  return h('div', { class: 'db-problems' }, read.problems.map((problem) => banner('bad', 'warn', problemWords(problem),
    problem.id && h('p', { class: 'small' }, h('a', { href: helpPath(problem.id) }, `What ${problem.id} means`)))));
}

/** The notice that the indexes of the database `name` are out of date, with the commands that bring them up to date. */
export function reindexNotice(name) {
  const lines = reindexLines(name);
  return banner('warn', 'warn', MISMATCH_WORDS,
    copyBox(lines[0], `Command that rebuilds the indexes of ${name}`),
    copyBox(lines[1], `Command that tells PostgreSQL the indexes of ${name} are up to date`));
}
