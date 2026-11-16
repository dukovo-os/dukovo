// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the palette finds of alerts: the cards of the Alerts page, for an
// administrator, who is the only one that has the page. The actions that
// change what alerts do are found as every action is, from admind's list.

/** The rows for a session that is an administrator (`admin`), or none. */
export function itemsOf(admin) {
  if (!admin) return [];
  return [
    { key: 'where', title: 'Where alerts go', detail: 'Email, webhooks and browsers, and what each is told of', href: '/alerts#channels' },
    { key: 'browser', title: 'Get alerts in this browser', detail: 'A notice on this device, even with the console closed', href: '/alerts#browser' },
    { key: 'phone', title: 'Set up a phone', detail: 'Open the console on a phone and get its alerts', href: '/alerts#phone' },
    { key: 'recent', title: 'Alerts now and lately', detail: 'What is active, and what was raised or cleared', href: '/alerts#active' },
  ];
}

export const providers = [{
  group: 'Alerts',
  kind: 'alerts',
  items: async (cx) => itemsOf(cx.session?.admin === true),
}];
