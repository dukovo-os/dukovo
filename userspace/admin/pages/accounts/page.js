// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A stand-in until this page is built: it shows the page's title and that
// the page comes later, so that the shell routes to every entry. The page
// that replaces this file has the same `show(view)`.

import { h } from '../../dom.js';

export async function show(view) {
  view.head({ title: 'Accounts' });
  view.main.append(h('p', { class: 'muted' }, 'Coming in this PR'));
}
