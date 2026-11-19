// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The list of accounts (design 7.4): the groups, the console's sign-in rules,
// the table of each person's account with what it can do and when it was last
// active, and Add an account…. The page reads the accounts when it opens,
// every 30 seconds while the tab is shown, and when a change says that
// something moved or the console is unlocked or locked; what is drawn changes
// only when what it says does, so that a link in hand stays in hand. An
// administrator who has not unlocked sees the list without Last active, and a
// reader sees Account and Can only.

import { fill, h, icon } from '../../dom.js';
import { PAGES } from '../../pages.js';
import { askToUnlock, isUnlocked } from '../../unlock.js';
import { atMostEvery } from '../overview/refresh.js';
import { avatar, bars } from './avatar.js';
import {
  accountHref, groupChips, keyCount, listRows, ruleWords, standingOf,
} from './model.js';
import { accountPath, soft } from './read.js';

/** How often the accounts are read again while the tab is shown. */
const READ_MS = 30_000;

/** The least time between two reads that an event asks for. */
const EVENT_GAP_MS = 2000;

/** Whether the page with the id `id` is in this console. */
const hasPage = (id) => PAGES.some((entry) => entry.id === id && entry.load !== null);

/**
 * What only an unlocked administrator reads: the list as root reads it,
 * which says which accounts are disabled and which have no password; when
 * each account was last active; and the keys of each account that has no
 * password (the list does not say whether it can sign in with a key).
 * Gives `{all, last, keys}`; `all` and `last` are null when the server does
 * not answer, and the page then shows the person's list.
 */
async function readAsAdministrator(view) {
  const [all, last] = await Promise.all([soft(view.api.get('/api/1/accounts/list/all')), soft(view.api.get('/api/1/sign-ins/last'))]);
  const whole = Array.isArray(all) ? all : null;
  const shown = await Promise.all((whole ?? []).filter((account) => account.password === 'none')
    .map(async (account) => [account.name, keyCount(await soft(view.api.get(accountPath(account.name))))]));
  return { all: whole, last: Array.isArray(last) ? new Map(last.map((one) => [one.name, one])) : null, keys: new Map(shown) };
}

/**
 * Shows the list. `add(names)` opens Add an account… for an administrator
 * who has unlocked; `names` are the accounts that there are.
 */
export async function showList(view, { add }) {
  const admin = view.session.admin;
  const addButton = h('button', { class: 'btn', type: 'button', 'data-action': 'add' }, icon('plus'), 'Add an account…');
  addButton.addEventListener('click', async () => {
    if (isUnlocked() || await askToUnlock(view.session, 'add an account')) add(accounts.map((account) => account.name));
  });
  view.head({ title: 'Accounts', commands: true, buttons: admin ? [addButton] : [] });

  let accounts = [];
  let latest = 0;
  let failed = false;
  let drawn = null;
  const content = h('div', { class: 'acc-list' }, h('div', { class: 'card acc-loading', 'aria-hidden': 'true' }, bars()));
  view.main.append(content);

  /** The line of groups, each with how many accounts are in it. Groups are not links: no page manages them. */
  const groupsLine = () => h('div', { class: 'acc-groups', role: 'group', 'aria-label': 'Groups' },
    h('span', { class: 'small muted' }, 'Groups:'),
    ...groupChips(accounts).map((group) => h('span', { class: 'acc-chip' }, `${group.name} · ${group.count}`)));

  /** The console's sign-in rules in a line, and where they are changed. */
  const rulesLine = (rules) => h('p', { class: 'small muted acc-rules' },
    icon('lock'), ' ',
    [rules, 'These are the console\'s rules; SSH has its own, on Security.'].filter(Boolean).join(' '),
    hasPage('security') && [' ', h('a', { href: '/security#sign-in' }, 'Change')]);

  /** One account's row of the table. */
  const rowOf = ({ account, own, disabled, can, last }) => h('tr', { class: own ? 'sel' : null },
    h('td', { 'data-label': 'Account' },
      h('div', { class: 'acc-who' },
        avatar(account),
        h('span', {}, h('a', { href: accountHref(account.name) }, h('b', {}, account.name)),
          own && [' ', h('span', { class: 'pill info' }, 'Your account')],
          account.full_name && h('small', {}, account.full_name)))),
    h('td', { 'data-label': 'Can' }, disabled ? h('span', { class: 'pill warn' }, can) : can),
    last !== null && h('td', { 'data-label': 'Last active' }, last));

  /** Draws what was read, unless it says what is drawn. */
  function draw({ last, keys, rules }) {
    const standing = standingOf(view.session, isUnlocked());
    const rows = listRows({
      accounts, last: standing === 'open' ? last : null, keys, self: view.session.name, zone: view.time.zone,
    });
    const key = JSON.stringify([rows, rules, standing]);
    if (key === drawn) return;
    drawn = key;
    const withLast = rows.length > 0 && rows[0].last !== null;
    const unlock = h('button', { class: 'btn small', type: 'button' }, 'Unlock');
    unlock.addEventListener('click', () => askToUnlock(view.session, 'see when each account was last active'));
    fill(content,
      groupsLine(),
      rulesLine(rules),
      standing === 'locked' && h('div', { class: 'banner info', role: 'status' }, icon('lock'),
        h('div', {}, h('p', {}, 'Unlock to see when each was last active.'), h('div', { class: 'btns' }, unlock))),
      h('div', { class: 'tbl-box' },
        h('table', { class: 'tbl stack' },
          h('caption', { class: 'sr-only' }, 'Accounts: what each can do and when it was last active'),
          h('thead', {}, h('tr', {}, h('th', { scope: 'col' }, 'Account'), h('th', { scope: 'col' }, 'Can'), withLast && h('th', { scope: 'col' }, 'Last active'))),
          h('tbody', {}, rows.map(rowOf))),
        h('p', { class: 'small muted acc-note' }, 'Service accounts made by apps (postgres, nginx and others) are on each app\'s page.')),
      rows.length === 1 && h('p', { class: 'muted' }, 'Add the people who use this server, so that each signs in as themselves.'));
  }

  /** Reads the accounts and what an administrator adds to them. Only the last read counts. */
  async function read() {
    latest += 1;
    const mine = latest;
    try {
      const [found, settings] = await Promise.all([view.api.get('/api/1/accounts'), soft(view.api.get('/api/1/security/sign-in'))]);
      const more = admin && isUnlocked() && Array.isArray(found)
        ? await readAsAdministrator(view)
        : { all: null, last: null, keys: new Map() };
      if (mine !== latest) return;
      accounts = more.all ?? (Array.isArray(found) ? found : []);
      if (failed) {
        failed = false;
        view.slot.replaceChildren();
      }
      content.hidden = false;
      draw({ last: more.last, keys: more.keys, rules: ruleWords(settings) });
    } catch (error) {
      if (mine !== latest || error?.name === 'AbortError') return;
      // What is drawn stays while the server does not answer; a first read that fails leaves nothing to show.
      if (drawn === null) content.hidden = true;
      if (failed) return;
      failed = true;
      view.problem(error, { buttons: [again] });
    }
  }

  const again = h('button', { class: 'btn small', type: 'button' }, 'Try again');
  again.addEventListener('click', () => {
    view.slot.replaceChildren();
    failed = false;
    content.hidden = false;
    read();
  });

  const soon = atMostEvery(read, EVENT_GAP_MS);
  view.signal.addEventListener('abort', soon.stop, { once: true });
  view.on('change', soon);
  view.on('unlock', soon);
  view.every(READ_MS, read);
  await read();
}
