// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the certificates' part of the Overview (design 3.5) makes of `GET
// /api/1/certificates`, the list of `cert list --json`: `[{name, kind,
// names, not_after, days_left, used_by, revoked?, ...}]`. `days_left` counts
// whole days from now to the end, and is negative once it has passed.

/** A certificate with this many days or fewer left is a thing to look at. */
export const WARN_DAYS = 14;

/** A certificate with this many days or fewer left is urgent. */
export const URGENT_DAYS = 3;

/** The list in `answer`, which is the array itself or an object that holds it. */
export const listOf = (answer) => (Array.isArray(answer) ? answer : Array.isArray(answer?.certificates) ? answer.certificates : []);

/** "nginx", "nginx and valkey", "nginx, valkey and web": the services that use a certificate, or ''. */
function usedBy(list) {
  if (!Array.isArray(list) || list.length === 0) return '';
  const names = list.length === 1 ? list[0] : `${list.slice(0, -1).join(', ')} and ${list[list.length - 1]}`;
  return `${names} ${list.length === 1 ? 'uses' : 'use'} it.`;
}

/** How long a certificate has left, in the words of a title: "ends today", "ends tomorrow", "ends in 9 days". */
function endsWords(days) {
  if (days === 0) return 'ends today';
  if (days === 1) return 'ends tomorrow';
  return `ends in ${days} days`;
}

/**
 * The needs of the Overview for the certificates of `answer`: one that was
 * revoked, one that has ended, and one with `WARN_DAYS` or fewer left. The
 * most urgent come first. `covers` says that the health check MET-107, which
 * warns of a certificate that ends soon, is not listed again beside these.
 */
export function needsOf(answer) {
  const found = [];
  for (const certificate of listOf(answer)) {
    const { name } = certificate;
    const used = usedBy(certificate.used_by);
    const base = { icon: 'certs', href: `/certificates/${encodeURIComponent(name)}`, at: null };
    if (certificate.revoked) {
      found.push({
        ...base, id: `certificate-revoked-${name}`, severity: 'bad', title: `${name} was revoked`,
        detail: ['The authority that made it has revoked it, so browsers do not trust it.', used, 'Renew it to get a new one.'].filter(Boolean).join(' '),
      });
    } else if (Number.isFinite(certificate.days_left) && certificate.days_left <= WARN_DAYS) {
      const days = certificate.days_left;
      const ended = days < 0;
      found.push({
        ...base,
        id: `certificate-ends-${name}`,
        severity: days <= URGENT_DAYS ? 'bad' : 'warn',
        title: ended ? `${name} has ended` : `${name} ${endsWords(days)}`,
        detail: [ended ? `It ended ${-days} ${days === -1 ? 'day' : 'days'} ago.` : '', used, certificate.kind === 'added' ? 'It was added by hand: add a new one.' : 'Renew it.'].filter(Boolean).join(' '),
        covers: ['MET-107'],
        days,
      });
    }
  }
  return found.sort((one, other) => (one.severity === other.severity ? 0 : one.severity === 'bad' ? -1 : 1)
    || (one.days ?? -Infinity) - (other.days ?? -Infinity)
    || one.id.localeCompare(other.id)).map(({ days, ...need }) => need);
}
