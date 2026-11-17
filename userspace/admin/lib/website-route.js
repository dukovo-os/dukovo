// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The addresses of the Websites pages, and the sites that use a certificate.
// Kept apart from website-model.js so that the Certificates page, which names
// the sites that use a certificate and links to them, does not read the rest.

/** The address of the page of the site `name`: one called `add` is written so that the page of Add a website is not taken for it. */
export const hrefOf = (name) => `/websites/${name === 'add' ? '%61dd' : encodeURIComponent(name)}`;

/**
 * What the part of the address after `/websites/` asks for: `{kind:
 * 'list'}`, `{kind: 'add'}` or `{kind: 'site', name}`.
 */
export function routeOf(rest) {
  const text = String(rest ?? '').replace(/\/+$/, '');
  if (text === '') return { kind: 'list' };
  if (text === 'add') return { kind: 'add' };
  try {
    return { kind: 'site', name: decodeURIComponent(text) };
  } catch {
    return { kind: 'site', name: text };
  }
}

/** The sites of an answer of `GET /api/1/websites` (the answer, or its list). */
export const sitesOf = (answer) => {
  const sites = Array.isArray(answer) ? answer : answer?.websites;
  return (Array.isArray(sites) ? sites : []).filter((site) => typeof site?.name === 'string' && site.name !== '');
};

/** The sites whose certificate is `name`: `[{name, first, href}]`, the first name being what a link says. */
export function sitesUsing(name, answer) {
  return sitesOf(answer).filter((site) => site.certificate === name)
    .map((site) => ({ name: site.name, first: site.names?.[0] ?? site.name, href: hrefOf(site.name) }));
}

/** The address of Add a website for `name`, with `certificate` chosen when it is one that is here; a wildcard is no name of a site. */
export function addHref(name = '', certificate = null) {
  const query = new URLSearchParams();
  if (name !== '' && !name.startsWith('*.')) query.set('name', name);
  if (certificate) query.set('certificate', certificate);
  const text = query.toString();
  return `/websites/add${text === '' ? '' : `?${text}`}`;
}
