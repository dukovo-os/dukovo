// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The sign-in rules (design 8.4): who gives a second step and how long a
// session lasts, as admin.toml(5) bounds them, with the form that changes
// them and says what follows before it is sent. They are the web console's
// rules and its API's; SSH signs in by its own settings. Not a trial: the
// rules take away nobody's way in.

import { fill, h } from '../../dom.js';
import { makeCard, unlocked } from './card.js';
import { RULES_NOTE, rulesOf, rulesWords } from './rules-model.js';

/**
 * The card. `reread()` reads the rules again after a change. Gives `{element,
 * set(answer)}`: `set` draws the rules of `admin settings --json`'s answer.
 */
export function makeSignIn(view, { reread }) {
  const card = makeCard('sign-in', 'Sign-in rules');
  const change = h('button', { class: 'btn small', type: 'button', hidden: true, 'data-action': 'sign-in-change' }, 'Change…');
  card.more.append(change);
  let rules = null;

  /** The form; its code is read when it is first asked for. */
  async function openChange() {
    if (rules === null || !(await unlocked(view, 'change the sign-in rules'))) return;
    const { openRulesForm } = await import('./sign-in-form.js');
    await openRulesForm(view, { rules, reread });
  }
  change.addEventListener('click', openChange);

  return {
    element: card.element,
    set(answer) {
      rules = rulesOf(answer);
      change.hidden = view.session.admin !== true;
      fill(card.body, ...rulesWords(rules).map((words) => h('p', {}, words)), h('p', { class: 'small muted' }, RULES_NOTE));
      card.ready();
    },
    fail(error) {
      view.problem(error, { slot: card.errors });
      fill(card.body, h('p', { class: 'muted' }, 'The sign-in rules could not be read.'));
      card.ready();
    },
  };
}
