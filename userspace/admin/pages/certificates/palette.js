// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the palette finds of certificates (design 3.5): each certificate
// certd keeps, with a state to filter by (`state:expiring`), and a row of its
// own to renew it ("Renew this-machine"). Until the Certificates page is
// there, Enter on a certificate shows it.

/** A certificate with this many days left or fewer is expiring. */
export const EXPIRING_DAYS = 14;

// The states of a certificate, the ones to look at first.
export const STATES = ['expiring', 'expired', 'revoked', 'valid'];

const days = (count) => `${count} ${count === 1 ? 'day' : 'days'}`;

/** The state word of a certificate from `cert list --json`. */
export function stateOf(certificate) {
  if (certificate.revoked === true) return 'revoked';
  if (typeof certificate.days_left !== 'number') return 'valid';
  if (certificate.days_left < 0) return 'expired';
  return certificate.days_left <= EXPIRING_DAYS ? 'expiring' : 'valid';
}

/** What a certificate's line says: when it ends, and what uses it. */
export function detailOf(certificate) {
  const left = certificate.days_left;
  const ends = typeof left !== 'number' ? '' : left < 0 ? `Expired ${days(-left)} ago` : left === 0 ? 'Expires today' : `Expires in ${days(left)}`;
  const users = Array.isArray(certificate.used_by) && certificate.used_by.length > 0 ? `used by ${certificate.used_by.join(', ')}` : '';
  return [ends, users].filter(Boolean).join(' · ');
}

/** The certificate rows of an answer of `GET /api/1/certificates`: its list, or an object that holds one. */
export function itemsOf(answer) {
  const certificates = Array.isArray(answer) ? answer : answer?.certificates;
  return (Array.isArray(certificates) ? certificates : [])
    .filter((certificate) => typeof certificate?.name === 'string' && certificate.name !== '')
    .map((certificate) => ({
      key: certificate.name,
      title: certificate.name,
      detail: detailOf(certificate),
      state: stateOf(certificate),
      href: `/certificates/${encodeURIComponent(certificate.name)}`,
      action: 'certificate.show',
      args: { name: certificate.name },
    }));
}

export const providers = [{
  group: 'Certificates',
  kind: 'certificate',
  filters: { state: STATES },
  items: async (cx) => itemsOf(await cx.cache('/api/1/certificates', 5000, () => cx.api.get('/api/1/certificates'))),
}];

export const phrases = {
  'certificate.renew': 'Renew {name}',
};
