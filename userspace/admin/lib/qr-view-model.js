// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The words around a QR code that hands a link to a phone, as plain values:
// the fingerprint in groups a person can compare, the time that is left, and
// what the phone's warning means. No DOM.

/** How many bytes of the fingerprint make one group. */
const GROUP = 8;

/**
 * The fingerprint `AB:CD:…` in groups of eight bytes, as a list of strings
 * (`AB:CD:EF:01:23:45:67:89`), so that two are compared a group at a time.
 * Anything that is not bytes in hex is one group as it came.
 */
export function fingerprintGroups(fingerprint) {
  const text = typeof fingerprint === 'string' ? fingerprint.trim() : '';
  const bytes = text.replace(/[\s:]/g, '').toUpperCase();
  if (!/^(?:[0-9A-F]{2})+$/.test(bytes)) return text === '' ? [] : [text];
  const pairs = bytes.match(/../g);
  const groups = [];
  for (let at = 0; at < pairs.length; at += GROUP) groups.push(pairs.slice(at, at + GROUP).join(':'));
  return groups;
}

/** "9 minutes 40 seconds" for `ms` milliseconds left, rounded up to the second; "" when none is left. */
export function leftWords(ms) {
  const seconds = Math.ceil(ms / 1000);
  if (!(seconds > 0)) return '';
  const minutes = Math.floor(seconds / 60);
  const rest = seconds % 60;
  const part = (count, noun) => `${count} ${noun}${count === 1 ? '' : 's'}`;
  return [minutes > 0 && part(minutes, 'minute'), rest > 0 && part(rest, 'second')].filter(Boolean).join(' ');
}

/** What the countdown says: the time left, or that the link has ended. */
export const countdownWords = (ms) => (leftWords(ms) === '' ? 'The link has ended.' : `The link ends in ${leftWords(ms)}.`);

/** What a person is told to do when the phone warns of the certificate: compare, and go on if the two are the same. */
export const WARNING_WORDS = 'Scan it with the phone\'s camera. The phone warns that it does not know this server\'s certificate: compare the fingerprint it shows with the one below, and go on if they are the same. The link works once, for 10 minutes.';

/** The name a screen reader gives the picture. */
export const pictureWords = (what) => `QR code of a link to ${what}, for 10 minutes`;
