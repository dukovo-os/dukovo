// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The steps of Add a database: make the role and the database, and for other
// machines let them connect, each as a step that says its state and stops the
// rest when it fails. The password is in the first step's answer, which is held
// here only until it is shown once; it is never drawn, stored or put in an
// address, and every way out of the page lets go of it. The page reads this
// file when Add the database is pressed.

import { fill, h, icon } from '../../dom.js';
import { hrefOf, secretOf } from '../../lib/database-model.js';
import { runSteps } from '../../lib/steps.js';
import { banner } from './cards.js';
import { showPassword } from './once.js';
import { connectionCard } from './settings.js';

/** What the leave guard says while the password is not yet shown. */
const leavingWords = (name) => `The password of ${name} is shown once, when the steps end. If you leave now it is lost, and you would have to make a new one.`;

/** The steps for a database `name` with the role `role`: `add` and `allow` are the arguments of their calls, `allow` null for a way that does not open the network. */
function stepsOf({ name, role, add, allow }) {
  const steps = [{
    title: `Make the role ${role} and the database ${name}`,
    action: 'databases.add',
    args: add,
    wait: 'answer',
    secret: true,
    ok: (answer) => secretOf(answer) !== null,
    words: {
      unlock: `add the database ${name}`,
      running: 'Adding the database',
      done: `${name} is added.`,
      failed: 'The database was not added',
      stands: `The database is made only when this step is done. If ${name} is in the list of databases anyway, make a new password on its page.`,
    },
  }];
  if (allow !== null) {
    steps.push({
      title: 'Let other machines connect',
      action: 'databases.allow',
      args: allow,
      wait: 'task',
      words: {
        unlock: `let other machines connect to ${name}`,
        running: 'Letting other machines connect',
        done: 'Other machines can connect.',
        failed: 'Other machines cannot connect yet',
        stands: `The database ${name} is made, and programs on this server can connect with its password. PostgreSQL's settings for other machines were not changed. Try again on its page.`,
        fix: `Open the page of ${name} and use Let other machines connect.`,
      },
    });
  }
  return steps;
}

/**
 * Runs Add a database in `host`: the steps, the password shown once and the
 * connection settings. `again()` puts the form back. Gives when the flow is
 * over; what it held is let go of by then, whatever way it ended.
 */
export async function runAdd(view, {
  host, name, role, way, site, machine, certificate, add, allow, again,
}) {
  const heading = h('h2', { id: 'db-run-h', tabindex: '-1' }, `Adding ${name}`);
  const run = runSteps(view, stepsOf({
    name, role, add, allow,
  }));
  const after = h('div', { class: 'db-after' });
  fill(host, h('section', { class: 'card', 'aria-labelledby': heading.id }, heading, run.element), after);
  view.leaving = () => leavingWords(name);

  const outcome = await run.finished;
  let secret = outcome.failed === 0 ? null : secretOf(outcome.answers[0]);
  // The answer of the first step holds the password: nothing keeps it but `secret`.
  outcome.answers.length = 0;
  const release = () => {
    secret = null;
    view.leaving = null;
  };
  view.signal.addEventListener('abort', release, { once: true });
  if (outcome.left || view.signal.aborted) {
    release();
    return;
  }

  // The first step failed: nothing was made, so there is no password to lose.
  if (secret === null) {
    release();
    fill(after, h('div', { class: 'btns' }, h('button', { class: 'btn default', type: 'button', onclick: again }, 'Back to the form'), h('a', { class: 'btn', href: '/databases' }, 'Cancel')));
    heading.focus();
    return;
  }

  const connects = way !== 'network' || outcome.done;
  const open = secret.password !== null;
  const own = secret.role;
  const kept = open ? await showPassword(view, {
    name, secret, way: connects ? way : 'local', machine,
  }) : true;
  release();
  const ways = way === 'network' ? ['local', ...(connects ? ['network'] : [])] : [way];
  const words = kept
    ? [`${name} is added.`, open ? 'Its password was shown once and is kept nowhere on this server.' : `It has no password: ${own} signs in as its account, through the socket.`]
    : [`${name} is added, but its password was not kept.`, 'Open its page and make a new password.'];
  fill(after,
    banner(kept && connects ? 'ok' : 'warn', kept && connects ? 'check' : 'warn', words[0], h('p', {}, words[1]),
      !connects && h('p', {}, 'Other machines cannot connect yet. Open its page to try again.')),
    connectionCard(view, {
      database: name, role: own, ways, machine, certificate,
    }),
    h('div', { class: 'btns' },
      h('a', { class: 'btn default', href: hrefOf(name) }, icon('db'), `Open ${name}`),
      h('a', { class: 'btn', href: '/databases/add' }, 'Add another'),
      site !== '' && h('a', { class: 'btn', href: `/websites/${encodeURIComponent(site)}` }, `Back to ${site}`)));
  heading.focus();
}
