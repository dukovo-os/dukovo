// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Accounts (design 7): `/accounts` is the list and `/accounts/NAME` is one
// account. Add an account… and an account's own page are read when they are
// first needed, so that the list opens with what it draws and no more.

import { nameOf } from './model.js';

export async function show(view) {
  const name = nameOf(view.rest);
  if (name === null) {
    const { showList } = await import('./list.js');
    await showList(view, {
      add: async (taken) => {
        const { openAdd } = await import('./add.js');
        const made = await openAdd(view, { taken });
        if (made !== null) view.go(`/accounts/${encodeURIComponent(made)}`);
      },
    });
    return;
  }
  const { showAccount } = await import('./account.js');
  await showAccount(view, { name, route: 'accounts' });
}
