// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// "Your account" (design 7.3 and 12.3): the person's own account as the
// Accounts pages draw it, with its display, time zone, second step, API
// tokens and the terminal's settings. `/accounts/NAME` for the person's own
// name shows the same page.

import { showAccount } from '../accounts/account.js';

export async function show(view) {
  await showAccount(view, { name: view.session.name, route: 'account' });
}
