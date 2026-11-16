// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A stand-in: the page's heading under its crumb, and a line that says it is being built.

import { h } from '../../dom.js';

export async function show(view) {
  view.head({ title: 'Websites', crumbs: [{ title: 'Apps', path: '/apps' }] });
  view.main.append(h('p', {}, 'This page is being built.'));
}
