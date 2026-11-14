// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The list of pages, by hand: one import and one array entry per page
// (design 3.2). An entry.js is data and functions that import, so this file
// loads no page code. A test fails when a folder in pages/ has no line here,
// or a line has no folder.
//
// The navigation's order is fixed here, ten apart, in the Main board's
// order, so that a later page slots in without a change to another:
// Overview 10, Logs 20, Reliability 30, Storage 40, Snapshots and backups
// 50, Networking 60, Firewall 70, Accounts 80, Services 90, Apps 100,
// Certificates 110, Security 120, Hardware 130, Updates 140, Terminal 150,
// and Report a problem 1000 at the bottom (`nav: {order: 1000, bottom:
// true}`). `nav: null` is a page reached by a link. Services and
// Certificates have none yet: their folders hold parts only, until C4 and
// C6 add the pages.

import account from './pages/account/entry.js';
import certificates from './pages/certificates/entry.js';
import overview from './pages/overview/entry.js';
import services from './pages/services/entry.js';
import setup from './pages/setup/entry.js';
import terminal from './pages/terminal/entry.js';
import welcome from './pages/welcome/entry.js';

// What an entry leaves out: no place in the navigation or the phone's tabs, a
// page for any signed-in person, nothing to find, no children, sections,
// style sheets or parts, and no page of its own.
const DEFAULTS = {
  nav: null, phone: null, needs: 'person', keywords: [], children: [], sections: [], css: [], load: null, parts: {},
};

export const PAGES = [overview, welcome, setup, account, terminal, services, certificates].map((entry) => ({ ...DEFAULTS, ...entry }));

/** The entry with `id`, or undefined. */
export const pageById = (id) => PAGES.find((entry) => entry.id === id);
