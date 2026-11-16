// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The steps of Add a website: install what is missing, add the site and, for
// a certificate from Let's Encrypt, get it and turn HTTPS on. Each step says
// its state and stops the rest when it fails; what stands is said under it.
// A site whose certificate could not be got is live over HTTP, and its page
// has Get the certificate. The page reads this file when Add the website is
// pressed.

import { fill, h, icon } from '../../dom.js';
import { liveUrl, outcomeOf, stepsOf } from '../../lib/website-model.js';
import { hrefOf } from '../../lib/website-route.js';
import { runSteps } from '../../lib/steps.js';
import { hasPage } from './read.js';

/** A banner of `kind` with `iconName`, the `words` and what else is in `more`. */
const banner = (kind, iconName, words, ...more) => h('div', { class: `banner ${kind}` }, icon(iconName), h('div', {}, h('p', {}, words), ...more));

/**
 * Runs Add a website in `host`. `form` is the page's, `missing` the apps to
 * install first (`{id, name, sha256}`), `database` whether the person asked for
 * a database too, and `again()` puts the form back. Gives when the flow is
 * over.
 */
export async function runAdd(view, {
  host, form, missing, database, again,
}) {
  const site = form.names[0];
  const steps = stepsOf(form, missing);
  const heading = h('h2', { id: 'web-run-h', tabindex: '-1' }, `Adding ${site}`);
  const list = h('div', { class: 'web-steps' });
  const after = h('div', { class: 'web-after' });
  const done = h('p', { class: 'small muted', hidden: true });
  fill(host, h('section', { class: 'card', 'aria-labelledby': heading.id }, heading, done, list), after);

  /** Runs the steps from `at` on; draws what ends it. */
  async function go(at) {
    const rest = steps.slice(at);
    done.hidden = at === 0;
    done.textContent = at === 0 ? '' : `Done already: ${steps.slice(0, at).map((one) => one.title).join(', ')}.`;
    fill(after);
    const run = runSteps(view, rest);
    fill(list, run.element);
    const result = await run.finished;
    if (result.left || view.signal.aborted) return;
    const outcome = outcomeOf(result, rest);
    const buttons = [];
    if (outcome.ok) {
      const url = liveUrl(site, form.how);
      fill(after, banner('ok', 'check', [`${site} is live: `, h('a', { href: url, target: '_blank', rel: 'noopener' }, url)]),
        h('div', { class: 'btns' },
          h('a', { class: 'btn default', href: hrefOf(site) }, 'Open its page'),
          database && hasPage('/databases/add') && h('a', { class: 'btn', href: `/databases/add?site=${encodeURIComponent(site)}` }, 'Make its database'),
          h('a', { class: 'btn', href: '/websites/add' }, 'Add another')),
        form.how === 'machine' && h('p', { class: 'small muted' }, 'Browsers warn until they trust this machine\'s certificate; Certificates says how.'));
      return;
    }
    if (outcome.again !== null) {
      const retry = h('button', { class: 'btn default', type: 'button', 'data-action': 'try-again' }, 'Try again');
      retry.addEventListener('click', () => go(at + outcome.again));
      buttons.push(retry);
    }
    if (!outcome.live) buttons.push(h('button', { class: 'btn', type: 'button', 'data-action': 'back', onclick: again }, 'Change the form'));
    else buttons.push(h('a', { class: 'btn', href: hrefOf(site) }, 'Open its page'));
    fill(after, banner('warn', 'warn', outcome.words), h('div', { class: 'btns' }, buttons));
    heading.focus();
  }

  await go(0);
}
