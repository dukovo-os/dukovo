// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Setting up a phone, as plain values: the address the code holds, whether
// push can work there, and the words of each step. The address is only the
// console's own, `https://HOST:PORT/alerts?phone=1`: it signs nobody in.

const normal = (fingerprint) => (typeof fingerprint === 'string' ? fingerprint.replace(/[:\s]/g, '').toLowerCase() : '');

/** Whether the console serves the machine's own certificate: both fingerprints are known and are the same, with colons and case left out. */
export const servesOwnCertificate = (served, machine) => normal(served) !== '' && normal(served) === normal(machine);

/** Whether `host` is a name that a certificate can name: not an address, not `localhost`, not a `.local` name. */
export const isNamedHost = (host) => typeof host === 'string' && host !== '' && !/^[\d.]+$/.test(host) && !host.includes(':')
  && !/^localhost$/i.test(host) && !/\.local$/i.test(host);

const privateV4 = (address) => /^(10\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.)/.test(address);
const privateV6 = (address) => /^f[cd][0-9a-f]{2}:/i.test(address);

/** The first private address of `addresses`, else the first address, else null. */
export const firstAddress = (addresses) => {
  const list = Array.isArray(addresses) ? addresses.filter((one) => typeof one === 'string' && one !== '') : [];
  return list.find((one) => privateV4(one) || privateV6(one)) ?? list[0] ?? null;
};

/**
 * What the code holds, from `GET /api/1/console/about`, the machine's own
 * certificate (`certificate.show this-machine`, or null), and the name this
 * page was reached by: `{url, push, own, fingerprint}`, or null when the
 * machine has no address. `push` is true when the address is a name that
 * another certificate names, the one case where a phone can get push.
 */
export function addressOf({ about, machine, hostname }) {
  const own = servesOwnCertificate(about?.fingerprint, machine?.fingerprint_sha256);
  const named = !own && normal(about?.fingerprint) !== '' && normal(machine?.fingerprint_sha256) !== '' && isNamedHost(hostname);
  const host = named ? hostname : firstAddress(about?.addresses);
  if (host === null) return null;
  const shown = host.includes(':') ? `[${host}]` : host;
  const port = Number.isInteger(about?.port) ? `:${about.port}` : '';
  return {
    url: `https://${shown}${port}/alerts?phone=1`, push: named, own, fingerprint: about?.fingerprint ?? null,
  };
}

/** Eight bytes a group, `AB:CD:…`, to be compared a group at a time. */
export function fingerprintGroups(fingerprint) {
  const bytes = normal(fingerprint).toUpperCase().match(/../g) ?? [];
  const groups = [];
  for (let at = 0; at < bytes.length; at += 8) groups.push(bytes.slice(at, at + 8).join(':'));
  return groups;
}

/** What step 2 says of the phone's warning, for the console's certificate: the machine's own, or another's. */
export const certificateWords = (own) => (own
  ? 'The phone warns that it does not know this server\'s certificate. Compare the fingerprint it shows with this one, and go on only if they are the same:'
  : 'The phone should trust this server\'s certificate. If it warns, compare the fingerprint it shows with this one, and go on only if they are the same:');

/** The five steps on a computer, in words; step 4 is `push` or the two ways. */
export const STEPS = Object.freeze({
  scan: 'Scan this with the phone\'s camera.',
  check: 'Check the certificate.',
  signIn: 'Sign in on the phone with your account.',
  alerts: 'Turn on alerts on the phone: the page there shows how.',
  noPush: 'Push notices need a certificate the phone trusts: ',
  home: 'Add it to the home screen from the browser\'s menu: it then opens like an app. No app store, no account.',
});

/** What the panel on the phone says of the home screen: a button when the browser offers one, else the menu. */
export const HOME_MENU_WORDS = 'Use the browser\'s menu: Add to the home screen.';
