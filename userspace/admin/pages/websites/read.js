// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Websites pages read and the small things they all draw from it:
// the sites with nginx's and PHP-FPM's states, the certificates certd keeps,
// a date in words, and a link to a page only when the console has it.

import { h } from '../../dom.js';
import { sitesOf } from '../../lib/website-route.js';
import { PAGES } from '../../pages.js';
import { matchPage } from '../../shell/route-model.js';

const SITES = '/api/1/websites';
const CERTIFICATES = '/api/1/certificates';

/** Whether the console has a page at `path`, so that a link to it is not a dead end. */
export const hasPage = (path) => matchPage(PAGES, path)?.entry.load != null;

/** `promise`'s answer, or null when it could not be had: what a page reads besides what it shows is not needed to show it. */
export const soft = (promise) => promise.catch((error) => {
  if (error?.name === 'AbortError') throw error;
  return null;
});

/** The answer of `GET /api/1/websites` made safe to read: `{nginx, phpFpm, sites}`, or null when it is not one. */
export function readOf(answer) {
  if (answer === null || typeof answer !== 'object' || Array.isArray(answer)) return null;
  return {
    nginx: answer.nginx ?? {}, phpFpm: answer.php_fpm ?? {}, sites: sitesOf(answer), answer,
  };
}

/**
 * Reads the sites and the certificates. A problem with the sites goes where
 * `slot` is; the certificates are only needed to say more of each site's
 * HTTPS, so a failure to read them gives null. Gives `{read, certificates}`,
 * `read` null when the sites could not be read, or null when the page was left.
 */
export async function readWebsites(view, slot) {
  const [sites, certificates] = await Promise.allSettled([view.api.get(SITES), view.api.get(CERTIFICATES)]);
  if (view.signal.aborted) return null;
  if (sites.status === 'rejected') view.problem(sites.reason, { slot });
  return {
    read: sites.status === 'fulfilled' ? readOf(sites.value) : null,
    certificates: certificates.status === 'fulfilled' && Array.isArray(certificates.value) ? certificates.value : null,
  };
}

/**
 * A date from the ISO text `text` as the console writes one ("Dec 14", and
 * "Dec 14, 2027" in another year than this one), in the person's time zone;
 * empty when `text` is no date. Written here and not taken from the
 * Certificates model, which brings that page's palette with it.
 */
export const dateOf = (view) => (text) => {
  const at = Date.parse(text ?? '');
  if (Number.isNaN(at)) return '';
  const format = (options) => new Intl.DateTimeFormat('en-US', { timeZone: view.time.zone, ...options });
  const sameYear = format({ year: 'numeric' }).format(at) === format({ year: 'numeric' }).format(Date.now());
  return format({ month: 'short', day: 'numeric', ...(sameYear ? {} : { year: 'numeric' }) }).format(at);
};

/** The link to the certificate `name` on Certificates. */
export const certificateLink = (name) => h('a', { href: `/certificates/${encodeURIComponent(name)}` }, name);
