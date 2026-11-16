// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// An account's API tokens (design 7.5 and 7.6): each with what it may do and
// when it runs out, and Revoke, which asks first, since whatever uses the
// token stops working. A person's own tokens are revoked by C2a's route
// and another account's by `accounts.token-revoke`. An administrator does
// not make tokens for another account; a person makes their own at a
// terminal, where the token is shown once.

import { h } from '../../dom.js';
import { tokenWords } from '../../lib/account-model.js';
import { copyBox } from '../snapshots/copy-box.js';
import { tokenRevokeCall } from './model.js';
import { openForm } from './open.js';

/** The command that makes a token, for a person's own account. */
export const TOKEN_COMMAND = 'admin token add NAME --scope services:read --days 30';

/** The route that revokes the person's own token, which is not an action: C2a's. */
const OWN_REVOKE = '/api/1/account/tokens/remove';

/**
 * The section of tokens. `ctx` is `{view, own, name, tokens, reload}`;
 * `tokens` are `user show`'s `{name, scopes, until_ms}`.
 */
export function tokensSection(ctx) {
  const {
    view, own, name, tokens, reload,
  } = ctx;
  const list = Array.isArray(tokens) ? tokens : [];

  /** Revokes `token`: through the action, or for the person's own by C2a's route. */
  async function revoke(token, confirm, errors) {
    if (!own) {
      const call = tokenRevokeCall({ name, token: token.name });
      return (await view.api.run(call.action, call.args, {
        button: confirm, words: { unlock: `revoke the token ${token.name}`, done: 'The token is revoked.' }, where: { slot: errors },
      })).ok;
    }
    try {
      await view.api.post(OWN_REVOKE, { name: token.name });
      view.notice('The token is revoked.', { kind: 'ok' });
      return true;
    } catch (error) {
      view.problem(error, { slot: errors });
      return false;
    }
  }

  const rows = list.map((token) => {
    const words = tokenWords(token, name, view.time.zone);
    const button = h('button', { class: 'btn small', type: 'button', 'aria-label': `Revoke the token ${token.name}` }, 'Revoke');
    button.addEventListener('click', async () => {
      const done = await openForm(view, {
        title: `Revoke the token ${token.name}?`,
        icon: 'warn',
        lead: ['Whatever uses it stops working.', h('p', { class: 'small muted' }, words.detail)],
        label: 'Revoke',
        danger: true,
        command: own ? null : tokenRevokeCall({ name, token: token.name }),
        submit: (confirm, errors) => revoke(token, confirm, errors),
      });
      if (done) reload();
    });
    return h('tr', {}, h('td', {}, h('b', {}, words.name), h('small', {}, words.detail)), h('td', { class: 'end' }, button));
  });

  return h('section', { class: 'acc-section', id: 'tokens', 'aria-labelledby': 'acc-tokens-h' },
    h('h3', { id: 'acc-tokens-h' }, 'API tokens'),
    rows.length > 0
      ? h('div', { class: 'tbl-box' }, h('table', { class: 'tbl acc-mini' }, h('caption', { class: 'sr-only' }, 'API tokens'), h('tbody', {}, rows)))
      : h('p', { class: 'muted' }, own ? 'You have no API token.' : `${name} has no API token.`),
    h('p', { class: 'small muted' }, 'A token can never do more than its owner, and every token runs out. Each change a token makes is in the journal under its name.'),
    own && [
      h('p', { class: 'muted' }, 'A script comes in with an API token, made at a terminal as you. The token is shown once; the scopes say what it may do.'),
      copyBox(TOKEN_COMMAND, 'The command that makes a token'),
    ]);
}
