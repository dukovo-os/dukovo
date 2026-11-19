// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The header (design 4.3 and 4.7): the brand, the host with its popover,
// Find, access, a trial's chip, Tasks, Help and the account menu. At phone
// width it holds the mark, the host, a find icon, an access icon, the chip
// and the Tasks icon while tasks run or need a look; Help and the account
// menu are in More.

import { post } from '../api.js';
import { fill, h, icon, marks, uniqueId } from '../dom.js';
import {
  accessControl, askToUnlock, confirmLock, isUnlocked, onUnlockChange, setUnlockSeconds, unlockMinutes,
} from '../unlock.js';
import { menu } from './menus.js';
import { THEME_CHOICES } from './prefs-model.js';

/** The words of the access icon at phone width, which has no room for the words of the wide control. */
export function accessWords(session, minutes) {
  if (minutes !== null) return `Unlocked, ${minutes} ${minutes === 1 ? 'minute' : 'minutes'} left`;
  return session.admin ? 'Limited access, unlock' : 'Limited access';
}

/** What the tip of a reader's access says (design 4.2). */
export const READER_TIP = 'Your account can look, and change your own things';

/** The host button; its popover (the addresses and the certificate's fingerprint) is shell/host-about.js, read when the button is first pressed. */
function hostPopover({ host, get }) {
  const id = uniqueId('host');
  const button = h('button', {
    class: 'host', type: 'button', 'aria-label': `Server ${host}`, 'aria-haspopup': 'dialog', 'aria-expanded': 'false', 'aria-controls': id,
  }, icon('server'), host, h('span', { class: 'chev', 'aria-hidden': 'true' }, '▼'));
  const element = h('span', { class: 'menu-box' }, button);
  let popover = null;
  button.addEventListener('click', async () => {
    popover ??= import('./host-about.js').then(({ aboutPopover }) => aboutPopover({ element, button, id, host, get }));
    (await popover).toggle();
  });
  return element;
}

/**
 * Makes the header from `shell`'s `session()`, `host`, `signal`, `prefs`,
 * `tasks`, `notice`, `openPalette()`, `openShortcuts()`, `openAbout()`,
 * `pageHelp()`, `openPageHelp()`, `signOut()` and `togglePages()`. Gives
 * `{element, tasksButton, pagesButton, accountItems, helpItems, trials}`.
 */
export function createHeader(shell) {
  const session = shell.session();

  const pagesButton = h('button', {
    class: 'topbtn pages-btn', type: 'button', 'aria-expanded': 'false', 'aria-controls': 'side-nav', onclick: () => shell.togglePages(),
  }, icon('menu'), 'Pages');

  const findWide = h('button', { class: 'find', type: 'button', 'aria-keyshortcuts': 'Control+K', onclick: () => shell.openPalette() },
    icon('search'), 'Search or run a command', h('span', { class: 'kbd', 'aria-hidden': 'true' }, 'Ctrl K'));
  const findIcon = h('button', { class: 'iconbtn find-icon', type: 'button', 'aria-label': 'Search or run a command', onclick: () => shell.openPalette() }, icon('search'));

  // The phone's access icon: the same states as the wide control. Locked, it
  // asks to unlock; unlocked, it offers the wide control's two buttons.
  const accessIcon = h('button', { class: 'iconbtn access-icon', type: 'button' });
  function drawAccess() {
    const minutes = unlockMinutes();
    accessIcon.replaceChildren(icon(minutes === null ? 'lock' : 'unlock'));
    accessIcon.setAttribute('aria-label', accessWords(session, minutes));
    accessIcon.classList.toggle('open', minutes !== null);
  }
  accessIcon.addEventListener('click', async () => {
    if (!session.admin) {
      shell.notice(READER_TIP);
      return;
    }
    if (!isUnlocked()) {
      await askToUnlock(session);
      return;
    }
    const { ask } = await import('./dialogs.js');
    const choice = await ask({
      title: 'Unlocked',
      words: accessWords(session, unlockMinutes()),
      iconName: 'unlock',
      choices: [{ label: 'Reset to 15 min', value: '/api/1/unlock/extend', kind: 'default' }, { label: 'Lock', value: '/api/1/lock' }, { label: 'Close', value: '' }],
    });
    if (!choice || (choice === '/api/1/lock' && !(await confirmLock()))) return;
    try {
      setUnlockSeconds((await post(choice)).unlock_seconds);
    } catch (error) {
      shell.notice(error.title, { kind: 'warn' });
    }
  });
  drawAccess();
  shell.signal.addEventListener('abort', onUnlockChange(drawAccess));

  // The chip is another module, read when a trial first runs.
  const trialSlot = h('span', { class: 'trial-slot', hidden: true });
  let chip = null;
  const trials = async (list) => {
    if (!chip && list.length === 0) return;
    chip ??= import('./trial-chip.js').then(({ trialChip }) => trialChip(trialSlot, shell));
    (await chip).show(list);
  };

  // Tasks: the count in a pill, and the words for a screen reader.
  const pillText = h('span', { class: 'count-pill', 'aria-hidden': 'true' });
  const tasksButton = h('button', {
    class: 'topbtn tasks-btn', type: 'button', 'aria-expanded': 'false', 'aria-controls': 'drawer', onclick: (event) => shell.tasks.open(event.currentTarget),
  }, icon('tasks'), h('span', { class: 'label' }, 'Tasks'), pillText);
  function drawPill(pill) {
    pillText.textContent = pill.count > 0 ? String(pill.count) : '';
    pillText.hidden = pill.count === 0;
    pillText.classList.toggle('warn', pill.warn);
    tasksButton.setAttribute('aria-label', pill.label);
    tasksButton.classList.toggle('quiet', pill.count === 0 && !pill.warn);
  }
  drawPill(shell.tasks.pill());
  shell.tasks.onPill(drawPill);

  /** The items of the Help menu, which the phone's More sheet lists too. */
  const helpItems = () => [
    { label: 'Keyboard shortcuts', onSelect: () => shell.openShortcuts() },
    shell.pageHelp() && { label: 'Help for this page', onSelect: () => shell.openPageHelp() },
    { label: 'What error messages mean', href: '/help/errors' },
    { label: 'About Dukovo Admin', onSelect: () => shell.openAbout() },
  ].filter(Boolean);
  const help = menu({ label: 'Help', content: ['Help'], items: helpItems });

  /** The items of the account menu, which the phone's More sheet lists too. */
  const accountItems = () => [
    { heading: session.full_name || session.name },
    { label: 'Your account', href: '/account' },
    { separator: true },
    { heading: 'Display' },
    ...THEME_CHOICES.map((choice) => ({
      label: choice.label,
      radio: true,
      checked: shell.prefs.value?.theme === choice.value,
      onSelect: () => shell.prefs.set({ theme: choice.value }).catch((error) => shell.notice(error.title, { kind: 'warn' })),
    })),
    { separator: true },
    { label: 'Sign out', onSelect: () => shell.signOut() },
  ];
  const account = menu({
    label: `Account of ${session.name}`,
    content: [session.name, h('span', { class: 'sr-only' }, ' account menu'), h('span', { 'aria-hidden': 'true' }, ' ▾')],
    items: accountItems,
  });

  const element = h('header', { class: 'top' },
    pagesButton,
    h('a', { class: 'brand', href: '/', 'aria-label': 'Dukovo Admin, Overview' }, ...marks(17, 22), h('span', { class: 'brand-name' }, 'Dukovo Admin')),
    hostPopover({ host: shell.host, get: shell.get }),
    findWide,
    h('span', { class: 'spacer' }),
    findIcon,
    h('span', { class: 'access-wide' }, accessControl(session)),
    accessIcon,
    trialSlot,
    tasksButton,
    h('span', { class: 'help-wide' }, help.element),
    h('span', { class: 'account-wide' }, account.element));
  return { element, tasksButton, pagesButton, accountItems, helpItems, trials };
}
