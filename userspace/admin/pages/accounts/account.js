// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// One account (design 7.3, 7.5 and 7.8), which is "Your account" for the
// person's own. What is shown depends on who looks (`viewOf`): the person's
// own account is whole from `account.show` whatever their standing; another
// account is whole to an unlocked administrator, from `accounts.show`; to a
// locked administrator and to a reader it is the account's row of the list and
// a line that says what they would see. The sessions are read on their own
// every 10 seconds, so that the rest of the page, and any dialog opened from
// it, is not drawn again each time. After an End the whole account is read
// again: its sessions at the machine come from `user show`, and ending a
// console terminal ends its shells there too.

import { fill, h, icon } from '../../dom.js';
import { accountLine, canWords, isDisabled, signedIn } from '../../lib/account-model.js';
import { askToUnlock, isUnlocked } from '../../unlock.js';
import { atMostEvery } from '../overview/refresh.js';
import { accountRows } from './change.js';
import { avatar, bars } from './avatar.js';
import { dangerSection } from './danger.js';
import { keysSection } from './keys.js';
import { keyCount, standingOf, viewOf } from './model.js';
import { ownCards } from './own.js';
import { accountPath, noSuchAccount, soft } from './read.js';
import { sessionsSection } from './sessions.js';
import { stepsRows } from './steps.js';
import { tokensSection } from './tokens.js';

/** How often the console sessions and terminals are read again while the page is shown. */
const SESSIONS_MS = 10_000;

/** The least time between two reads that an event asks for. */
const EVENT_GAP_MS = 2000;

/** The grey bars that stand where the rows are read. */
const frames = () => h('div', { class: 'card acc-loading', 'aria-hidden': 'true' }, bars(6));

/** The words of an account that is not there (ACC-101's), with the way back to the list. */
const missing = (name) => h('section', { class: 'card' },
  h('p', {}, `There is no account called ${name}.`),
  h('div', { class: 'btns' }, h('a', { class: 'btn', href: '/accounts' }, 'Back to Accounts')));

/** The top of the card: the account's circle, its name and its line. */
const top = (account, line) => h('div', { class: 'acc-who acc-top' },
  avatar(account, true),
  h('div', {}, h('h2', { id: 'acc-name' }, account.name), h('span', { class: 'muted' }, line)));

/**
 * Shows the account `name`. `route` is `account` for `/account`, which
 * names the page "Your account", and `accounts` for `/accounts/{name}`.
 */
export async function showAccount(view, { name, route }) {
  const own = name === view.session.name;
  view.head({
    title: route === 'account' || own ? 'Your account' : name,
    crumbs: route === 'account' ? [] : [{ title: 'Accounts', path: '/accounts' }],
    commands: true,
  });

  let latest = 0;
  let model = null;
  let drawnBody = null;
  let drawnSessions = null;
  let sessions = null;
  let ssh = null;

  const body = h('div', { class: 'acc-body' }, frames());
  const sessionsBox = h('div', { class: 'acc-sessions' });
  const extra = h('div', { class: 'acc-extra' });
  view.main.append(h('div', { class: 'acc-account' }, body, sessionsBox, extra));
  if (own) fill(extra, ...ownCards(view));

  /** Reads everything again and draws it. */
  const reload = () => load();

  /** What the sessions section says now, drawn when it differs from what is there. */
  function drawSessions() {
    if (model === null || (model.kind !== 'own' && model.kind !== 'full')) {
      drawnSessions = null;
      fill(sessionsBox);
      return;
    }
    const rows = signedIn({
      sessions: sessions?.sessions ?? [], terminals: sessions?.terminals ?? [], machine: model.machine,
    }, view.time.zone);
    const key = JSON.stringify([rows, sessions === null, model.kind]);
    if (key === drawnSessions) return;
    drawnSessions = key;
    fill(sessionsBox, h('div', { class: 'card' }, sessionsSection({
      view, kind: model.kind, name, rows, unread: sessions === null, reload,
    })));
  }

  /** The card of an account that the person sees whole. */
  function wholeCard() {
    const {
      kind, account, keys, steps, tokens,
    } = model;
    const ctx = {
      view, kind, own, name, account, steps, keys, tokens, sshPasswords: ssh, admin: account.admin === true, reload,
    };
    const rows = accountRows(ctx);
    const second = stepsRows(ctx);
    return h('section', { class: 'card acc-card', 'aria-labelledby': 'acc-name' },
      top(account, accountLine(account, view.time.zone)),
      isDisabled(account) && h('div', { class: 'banner warn', role: 'status' }, icon('lock'), h('div', {}, h('p', {}, `${name} is disabled: nothing signs in to the account.`))),
      h('dl', { class: 'rows' }, ...rows.name, ...rows.groups, ...rows.admin, ...rows.console, ...rows.password, ...second, ...rows.shell),
      keysSection(ctx),
      tokensSection(ctx),
      (kind === 'full' || account.admin) && dangerSection(ctx));
  }

  /** The row of the list for an account that is not shown whole, with what the person would see if they could. */
  function rowCard() {
    const { kind, account } = model;
    const unlock = h('button', { class: 'btn small', type: 'button' }, 'Unlock');
    unlock.addEventListener('click', () => askToUnlock(view.session, `see ${name}'s keys, second steps, tokens and sessions`));
    return h('section', { class: 'card acc-card', 'aria-labelledby': 'acc-name' },
      top(account, [canWords(account), account.full_name && ` · ${account.full_name}`].filter(Boolean).join('')),
      kind === 'locked'
        ? h('div', { class: 'banner info', role: 'status' }, icon('lock'), h('div', {}, h('p', {}, `Unlock to see ${name}'s keys, second steps, tokens and sessions.`), h('div', { class: 'btns' }, unlock)))
        : h('p', { class: 'muted' }, 'Administrators see the rest.'));
  }

  /** Draws the account unless what is drawn says the same. */
  function draw() {
    const key = JSON.stringify([model, view.time.zone, ssh]);
    if (key !== drawnBody) {
      drawnBody = key;
      fill(body, model.missing ? missing(name) : (model.kind === 'own' || model.kind === 'full') ? wholeCard() : rowCard());
    }
    drawSessions();
  }

  /** Reads the sessions route, which can fail without the page failing. */
  async function refreshSessions() {
    if (model === null || (model.kind !== 'own' && model.kind !== 'full')) return;
    const found = await soft(view.api.get(accountPath(name, '/sessions')));
    sessions = found && Array.isArray(found.sessions) ? found : null;
    drawSessions();
  }

  /** Whether the account's only key could be the only way in, so that it is worth asking if SSH takes passwords. */
  const loneKey = (shown) => keyCount(shown) === 1 && shown.account.password === 'set' && !shown.account.admin && !shown.account.console;

  /** Reads what the person may see of the account and draws it. Only the last read counts. */
  async function load() {
    latest += 1;
    const mine = latest;
    const kind = viewOf({ own, standing: standingOf(view.session, isUnlocked()) });
    try {
      let next;
      if (kind === 'own' || kind === 'full') {
        const shown = await view.api.get(kind === 'own' ? '/api/1/account/show' : accountPath(name));
        const found = await Promise.all([
          loneKey(shown) ? soft(view.api.get('/api/1/ssh')) : null,
          soft(view.api.get(accountPath(name, '/sessions'))),
        ]);
        if (mine !== latest) return;
        ssh = found[0]?.passwords ?? null;
        sessions = found[1] && Array.isArray(found[1].sessions) ? found[1] : null;
        next = {
          kind, account: shown.account, keys: shown.keys ?? [], steps: shown.steps ?? null, tokens: shown.tokens ?? [], machine: shown.sessions ?? [],
        };
      } else {
        const list = await view.api.get('/api/1/accounts');
        if (mine !== latest) return;
        const account = (Array.isArray(list) ? list : []).find((one) => one.name === name);
        next = account ? { kind, account } : { kind, missing: true };
      }
      model = next;
      view.slot.replaceChildren();
      draw();
    } catch (error) {
      if (mine !== latest || error?.name === 'AbortError') return;
      if (noSuchAccount(error)) {
        model = { kind, missing: true };
        draw();
        return;
      }
      if (model === null) fill(body);
      view.problem(error, { buttons: [again] });
    }
  }

  const again = h('button', { class: 'btn small', type: 'button' }, 'Try again');
  again.addEventListener('click', () => {
    view.slot.replaceChildren();
    fill(body, frames());
    drawnBody = null;
    load();
  });

  const soon = atMostEvery(load, EVENT_GAP_MS);
  view.signal.addEventListener('abort', soon.stop, { once: true });
  view.on('change', soon);
  view.on('unlock', soon);
  view.every(SESSIONS_MS, refreshSessions);
  await load();
}
