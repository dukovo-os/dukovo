// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Check the sites: nginx's own check of the sites as they are, run as nginx,
// which only an administrator may ask, and what it found, with nginx's own
// line and, for settings that lack the two lines the sites need, the lines and
// where they go. The list and a site's page both draw it.

import { fill, h, icon } from '../../dom.js';
import { checkedProblems, checkWords, confRows } from '../../lib/website-model.js';
import { helpPath } from '../../lib/problem-model.js';
import { copyBox } from '../snapshots/copy-box.js';

/** The unlock words and the notice of the check. */
const WORDS = {
  unlock: 'check the websites', running: 'Checking the websites', busy: 'Checking…', done: 'The check is done.',
};

/** A banner of `tone` (`ok`, `warn` or `bad`) with the `words`, and what else is in `more`. */
const banner = (tone, words, ...more) => h('div', { class: `banner ${tone === 'ok' ? 'ok' : tone}` },
  icon(tone === 'ok' ? 'check' : 'warn'), h('div', {}, h('p', {}, words), ...more));

/** The id of an error as a link to what it means. */
const idLink = (id) => id && h('p', { class: 'small' }, h('a', { href: helpPath(id) }, `What ${id} means`));

/**
 * The check for `view`: `button` runs it and `element` shows what it found.
 * `errors` is where a refusal goes; `onResult(answer)` is called with the
 * answer, which `problemsOf` gives the problems of each site of. A reader has
 * no button: the check runs as nginx, which only an administrator may ask.
 */
export function makeCheck(view, { errors, onResult = () => {} }) {
  const element = h('div', { class: 'web-check' });
  const button = h('button', { class: 'btn', type: 'button', 'data-action': 'check' }, 'Check the sites');
  let last = null;

  function draw() {
    const said = checkWords(last);
    fill(element,
      said && banner(said.tone, said.words, said.line && h('pre', { class: 'cmdtext', tabindex: '0', 'aria-label': 'nginx\'s own line' }, said.line)),
      confRows(last).map((row) => banner(row.tone, row.words, copyBox(row.lines.join('\n'), `The lines to add: ${row.key}`), idLink(row.id))));
  }

  async function run() {
    const ran = await view.api.run('websites.check', {}, { button, where: { slot: errors }, words: WORDS });
    if (!ran.ok || ran.result === null || typeof ran.result !== 'object') return;
    last = ran.result;
    draw();
    onResult(last);
  }

  button.addEventListener('click', run);
  return {
    button,
    element,
    run,
    get result() {
      return last;
    },
    /** The problems nginx's check found for each site: `{site: [{id, words}]}`. */
    problems: () => checkedProblems(last),
  };
}
