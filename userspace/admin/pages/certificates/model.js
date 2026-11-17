// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Certificates page says of the certificates certd keeps, as plain
// values: each row of the list from `cert list --json` (who made it, when it
// ends, who uses it, what can be done with it), the line about the account at
// the authority, which certificates the console may serve, and the failure
// of a renewal. Nothing here touches the page.

import { EXPIRING_DAYS, stateOf } from './palette.js';

/** The name of the certificate this server makes for itself. */
export const SELF = 'this-machine';

/** How many days before the end certd warns about a certificate it cannot renew: one added by hand, or one proved by DNS. */
export const BY_HAND_DAYS = 21;

/** How many days before the end certd warns about one whose renewal keeps failing. */
export const FAILING_DAYS = EXPIRING_DAYS;

/** From how many days before the end the warning is critical. */
export const CRITICAL_DAYS = 7;

/** What certd does, which the line under the head says. */
export const CERTD_WORDS = 'certd renews each Let\'s Encrypt certificate on the day the authority suggests (ARI), or when two thirds of its life are gone, and warns you from 14 days before it runs out if a renewal keeps failing.';

/** Where the warnings go. */
export const WARNING_WORDS = 'Warnings go to the alerts (email, webhooks and this console).';

const DAY_SECONDS = 86_400;
const LOCALE = 'en-US';

/** The Unix time in seconds of the ISO text `text`, or null. */
export function secondsOf(text) {
  const at = Date.parse(text ?? '');
  return Number.isNaN(at) ? null : Math.floor(at / 1000);
}

/** The year of the Unix time `ms` in `zone`. */
const yearOf = (zone, ms) => Number(new Intl.DateTimeFormat(LOCALE, { timeZone: zone, year: 'numeric' }).format(ms));

/** "Jan 29", and "Jan 25, 2029" in a year that is not this one; `time` is the person's (`zone`), `now` in milliseconds. */
export function dateWords(seconds, time, now = Date.now()) {
  if (!Number.isFinite(seconds)) return '';
  const ms = seconds * 1000;
  const options = yearOf(time.zone, ms) === yearOf(time.zone, now) ? {} : { year: 'numeric' };
  return new Intl.DateTimeFormat(LOCALE, {
    timeZone: time.zone, month: 'short', day: 'numeric', ...options,
  }).format(ms);
}

/** "1 day", "2 days". */
export const daysWords = (count) => `${count} ${count === 1 ? 'day' : 'days'}`;

/** The parts `[type, value]` of a distinguished name as RFC 4514 writes it ("C=US, O=Let's Encrypt, CN=R11"). */
export function dnParts(name) {
  const unescape = (text) => text
    .replace(/(?:\\[0-9a-fA-F]{2})+/g, (run) => new TextDecoder().decode(Uint8Array.from(run.match(/[0-9a-fA-F]{2}/g), (byte) => parseInt(byte, 16))))
    .replace(/\\(.)/g, '$1');
  return String(name ?? '').split(/(?<!\\)[,+]\s*/).map((part) => {
    const at = part.indexOf('=');
    return at < 0 ? null : [part.slice(0, at).trim().toUpperCase(), unescape(part.slice(at + 1).trim())];
  }).filter(Boolean);
}

/** The first value of `type` in the parts `parts`, or null. */
const partOf = (parts, type) => parts.find(([one, value]) => one === type && value !== '')?.[1] ?? null;

/** Whether the organisation `name` is Let's Encrypt's, which also signs with a test authority that says so. */
export const isLetsEncrypt = (name) => /let'?s encrypt/i.test(name ?? '');

/** The kind of a certificate as the page sorts it: `machine` (this server's own), `added` or `authority`. */
export function kindOf(certificate) {
  if (certificate.kind === 'self-signed' || certificate.name === SELF) return 'machine';
  return certificate.kind === 'added' ? 'added' : 'authority';
}

/** The words of the From column: `{words, note}`; `note` says when it is not from an authority this server trusts. */
export function fromWords(certificate) {
  if (kindOf(certificate) === 'machine') return { words: 'This server', note: null };
  const parts = dnParts(certificate.issuer);
  const organisation = partOf(parts, 'O');
  let words = organisation ?? partOf(parts, 'CN') ?? String(certificate.issuer ?? '');
  if (isLetsEncrypt(words)) words = /staging/i.test(words) ? 'Let\'s Encrypt (test authority)' : 'Let\'s Encrypt';
  return { words, note: certificate.trusted === false ? 'Not from an authority this server trusts' : null };
}

/** How many names there are beyond the first. */
export const moreNames = (certificate) => Math.max(0, (certificate.names ?? []).length - 1);

/**
 * Whether certd cannot renew the certificate by itself: one added by hand,
 * and one from an authority that certd has no renewal day for while it is
 * near its end, which is how one proved by DNS looks. `byDns` is the set of
 * names a refused renewal said so of.
 */
export function cannotRenew(certificate, byDns = new Set()) {
  const kind = kindOf(certificate);
  if (kind === 'added') return true;
  if (kind !== 'authority' || certificate.revoked === true) return false;
  if (byDns.has(certificate.name)) return true;
  return !certificate.renews && typeof certificate.days_left === 'number' && certificate.days_left <= BY_HAND_DAYS;
}

/**
 * What the Good until column says: `{date, note, tone}`. `date` is the day it
 * ends; `note` is the second line, "renews Dec 30 by itself" or "11 days left,
 * cannot renew by itself"; `tone` is `bad`, `warn` or null.
 */
export function endWords(certificate, time, { now = Date.now(), byDns = new Set() } = {}) {
  const ends = secondsOf(certificate.not_after);
  const date = ends === null ? '' : dateWords(ends, time, now);
  const left = certificate.days_left;
  if (certificate.revoked === true) return { date, note: 'revoked', tone: 'bad' };
  if (typeof left === 'number' && left < 0) return { date, note: `ended ${daysWords(-left)} ago`, tone: 'bad' };
  if (kindOf(certificate) === 'machine') return { date, note: null, tone: null };
  const critical = typeof left === 'number' && left <= CRITICAL_DAYS;
  if (cannotRenew(certificate, byDns)) {
    const words = typeof left === 'number' ? `${daysWords(left)} left, cannot renew by itself` : 'cannot renew by itself';
    return { date, note: words, tone: typeof left === 'number' && left <= BY_HAND_DAYS ? (critical ? 'bad' : 'warn') : null };
  }
  const renews = secondsOf(certificate.renews);
  if (renews !== null) return { date, note: `renews ${dateWords(renews, time, now)} by itself`, tone: null };
  return { date, note: null, tone: typeof left === 'number' && left <= FAILING_DAYS ? (critical ? 'bad' : 'warn') : null };
}

/** The AB:CD:… form of the SHA-256 fingerprint that browsers show, from the hex that `cert` gives (either form). */
export function fingerprintWords(text) {
  const hex = String(text ?? '').replace(/[^0-9a-fA-F]/g, '').toUpperCase();
  return hex.length === 0 ? '' : hex.match(/.{1,2}/g).join(':');
}

/** The service a name in Used by links to. */
export const serviceHref = (name) => `/services/${encodeURIComponent(name)}`;

/** The words of the Used by column when no service uses the certificate: `null` when one does. */
export function unusedWords(certificate) {
  if ((certificate.used_by ?? []).length > 0) return null;
  return kindOf(certificate) === 'machine' ? 'kept for use by name on the local network' : 'no service uses it yet';
}

/**
 * Whether the name `name` of a certificate covers the address `host`, as a
 * browser checks it: an IP address is the same address, a DNS name is the
 * same in small and capital letters, and a wildcard stands for the left-most
 * word alone.
 */
export function covers(name, host) {
  const bare = String(host ?? '').replace(/^\[(.*)\]$/, '$1').replace(/\.$/, '').toLowerCase();
  const wanted = String(name ?? '').replace(/^\[(.*)\]$/, '$1').toLowerCase();
  if (bare === '' || wanted === '') return false;
  const address = (text) => {
    if (/^\d{1,3}(\.\d{1,3}){3}$/.test(text)) return text;
    if (!text.includes(':')) return null;
    try {
      return new URL(`http://[${text}]/`).hostname;
    } catch {
      return null;
    }
  };
  const there = address(bare);
  if (there !== null) return address(wanted) === there;
  if (wanted.startsWith('*.')) {
    const at = bare.indexOf('.');
    return at > 0 && !bare.slice(0, at).includes('*') && bare.slice(at + 1) === wanted.slice(2);
  }
  return !wanted.includes('*') && wanted === bare;
}

/** Whether one of `names` covers `host`. */
export const namesCover = (names, host) => (names ?? []).some((name) => covers(name, host));

/**
 * What "Use for this console" is on a row: `{offer, words}`. `offer` is
 * `use` (a button), `again` (this server's own, which the console serves
 * again), `open` (the page must be opened at a name the certificate has
 * first, `url`), `none` (it does not name this machine), or null (nothing to
 * say: it is served already, it was revoked, or it has ended). `host` is the
 * address this browser reaches the console by, `full` the machine's full
 * name, `port` the console's, `serving` the certificate it serves now.
 * admind refuses a certificate that does not name `host` (ADM-141), this
 * server's own too, so each is offered only where it names it.
 */
export function useOffer(certificate, {
  host, full, port, serving,
}) {
  if (certificate.name === serving || certificate.revoked === true || (typeof certificate.days_left === 'number' && certificate.days_left < 0)) return { offer: null, words: '' };
  if (namesCover(certificate.names, host)) {
    return kindOf(certificate) === 'machine' ? { offer: 'again', words: 'Use this server\'s own certificate again' } : { offer: 'use', words: 'Use for this console' };
  }
  const shown = port && String(port) !== '443' ? `:${port}` : '';
  if (full && namesCover(certificate.names, full)) {
    const url = `https://${full}${shown}/`;
    return { offer: 'open', url, words: `Open the console at ${url} and choose it there` };
  }
  return { offer: 'none', words: 'This certificate does not name this machine. Give the machine that name first (Name and domain, below) to use it for the console.' };
}

/**
 * What a row can be asked: `{buttons, more, removeNote}`, each an id the
 * page draws as a button (`show`, `make-anew`, `renew`, `renew-dns`,
 * `replace`, `use`, `again`, `revoke`, `remove`); `more` are those that go
 * under More. `admin` says whether the person may change certificates; a
 * reader can only look.
 */
export function actionsOf(certificate, {
  admin, serving, byDns = new Set(),
}) {
  const kind = kindOf(certificate);
  const users = certificate.used_by ?? [];
  const revoked = certificate.revoked === true;
  const serves = certificate.name === serving;
  const removeNote = users.length === 0 ? null
    : `Remove is offered when no service uses it: ${users.join(', ')} ${users.length === 1 ? 'does' : 'do'}.`;
  const remove = users.length === 0 ? ['remove'] : [];
  if (!admin) return { buttons: ['show'], more: [], removeNote: null };
  if (kind === 'machine') return { buttons: ['show', 'make-anew'], more: serves ? [] : ['again'], removeNote: null };
  if (kind === 'added') {
    return { buttons: ['replace'], more: ['show', ...(serves || revoked ? [] : ['use']), ...remove], removeNote };
  }
  if (revoked) return { buttons: ['show'], more: remove, removeNote };
  return {
    buttons: [byDns.has(certificate.name) ? 'renew-dns' : 'renew'],
    more: ['show', ...(serves ? [] : ['use']), 'revoke', ...remove],
    removeNote,
  };
}

/** The words of the buttons `actionsOf` names. */
export const ACTION_WORDS = Object.freeze({
  show: 'Show fingerprint',
  'make-anew': 'Make it anew…',
  renew: 'Renew now',
  'renew-dns': 'Renew by DNS…',
  replace: 'Replace…',
  use: 'Use for this console',
  again: 'Use this server\'s own certificate again',
  revoke: 'Revoke…',
  remove: 'Remove…',
});

/**
 * The rows of the list, in the order the store gives, from `cert list
 * --json`'s `list`. `failures` is `failuresOf`'s; `serving` the certificate
 * the console serves, or null while that is not known.
 */
export function rowsOf(list, time, {
  failures = new Map(), serving = null, byDns = new Set(), now = Date.now(),
} = {}) {
  return (Array.isArray(list) ? list : [])
    .filter((certificate) => typeof certificate?.name === 'string' && certificate.name !== '')
    .map((certificate) => {
      const kind = kindOf(certificate);
      const names = Array.isArray(certificate.names) ? certificate.names : [];
      return {
        name: certificate.name,
        kind,
        first: names[0] ?? certificate.name,
        more: moreNames(certificate),
        names,
        console: certificate.name === serving,
        from: fromWords(certificate),
        end: endWords(certificate, time, { now, byDns }),
        state: stateOf(certificate),
        usedBy: certificate.used_by ?? [],
        unused: unusedWords(certificate),
        failure: failures.get(certificate.name) ?? null,
        certificate,
      };
    });
}

/** The line about the account at the authority: "Let's Encrypt account: made on Nov 2, with no contact email", or null when no certificate has one. */
export function accountWords(list, time, now = Date.now()) {
  const account = (Array.isArray(list) ? list : []).map((certificate) => certificate?.account).find((one) => one && Number.isFinite(one.made_ms));
  if (!account) return null;
  return `Let's Encrypt account: made on ${dateWords(Math.floor(account.made_ms / 1000), time, now)}, with ${account.contact === true ? 'a contact email' : 'no contact email'}`;
}

/** The names a person can give a certificate asked for under `first`: the Domain the form suggests, from the machine's name; null when it has no domain. */
export const suggestedDomain = (machine) => (machine?.domain ? `${machine.first}.${machine.domain}` : null);
