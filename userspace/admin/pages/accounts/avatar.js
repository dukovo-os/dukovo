// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The marks that the Accounts pages share: an account's letter in a circle,
// and the grey bars that stand where something is being read.

import { h } from '../../dom.js';
import { initialOf, shadeOf } from '../../lib/account-model.js';

/** A letter in a circle, which stands for the account and is not read: its name is beside it. */
export const avatar = (account, big = false) => h('span', {
  class: `acc-avatar shade-${shadeOf(account)}${big ? ' big' : ''}`, 'aria-hidden': 'true',
}, initialOf(account));

/** `count` grey bars. They are still, and nothing reads them. */
export const bars = (count = 5) => Array.from({ length: count }, () => h('span', { class: 'acc-bar' }));
