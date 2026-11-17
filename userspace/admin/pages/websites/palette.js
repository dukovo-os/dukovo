// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the palette finds of websites: each site ("shop.example.org · PHP ·
// HTTPS") with a state to filter by (`state:http-only`, `state:by-hand`), Add a
// website, and for each site that has no certificate a row that goes to its
// page to get one. A site has the phrase "Remove {name}". The palette reads
// this file when it opens.

import { hrefOf, sitesOf } from '../../lib/website-route.js';

/** The states a site is found by. */
export const STATES = ['http-only', 'by-hand'];

/** The words after a site's name: what it serves and how. */
export function detailOf(site) {
  const serves = site.kind === 'php' ? 'PHP' : site.kind === 'static' ? 'Files' : 'By hand';
  return [serves, site.https ? 'HTTPS' : 'HTTP only'].join(' · ');
}

/** The state word of a site, or ''. A site that was changed by hand is that first, since what it says of HTTPS may not be the console's to change. */
export function stateOf(site) {
  if (site.by_hand || site.added_by_hand) return 'by-hand';
  return site.https ? '' : 'http-only';
}

/** The rows for the answer of `GET /api/1/websites`. */
export function itemsOf(answer) {
  return sitesOf(answer).map((site) => ({
    key: site.name,
    title: site.names?.[0] ?? site.name,
    detail: detailOf(site),
    state: stateOf(site),
    href: hrefOf(site.name),
    args: { name: site.name },
  }));
}

/** The things the page does, which only an administrator does: add a website, and get a certificate for a site that has none. */
export function pageItems(answer, admin) {
  if (!admin) return [];
  return [
    {
      key: 'add', title: 'Add a website', detail: 'A name, what it serves, a certificate and a folder', href: '/websites/add',
    },
    ...sitesOf(answer).filter((site) => !site.https && site.kind !== 'other' && !site.by_hand && !site.added_by_hand).map((site) => ({
      key: `certificate:${site.name}`,
      title: `Get a certificate for ${site.names?.[0] ?? site.name}`,
      detail: 'From Let\'s Encrypt, then serve it over HTTPS',
      href: hrefOf(site.name),
    })),
  ];
}

export const providers = [{
  group: 'Websites',
  kind: 'website',
  filters: { state: STATES },
  items: async (cx) => itemsOf(await cx.cache('/api/1/websites', 5000, () => cx.api.get('/api/1/websites'))),
}, {
  group: 'Websites',
  kind: 'website',
  items: async (cx) => (cx.session?.admin === true
    ? pageItems(await cx.cache('/api/1/websites', 5000, () => cx.api.get('/api/1/websites')), true)
    : []),
}];

// The site's name is what the change takes.
export const phrases = {
  'websites.remove': 'Remove {name}',
};
