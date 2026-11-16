// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the page can tell of a certificate and a key before they are sent,
// without reading a certificate: that each box holds what it should, and
// that both fit in the one document the action takes. `cert add` does every
// real check, and its refusal comes back with its words and ID.

/** The most bytes of the document the action takes: the key and the chain together. */
export const PEM_MAX = 64 * 1024;

const CERTIFICATE = /-----BEGIN CERTIFICATE-----[\s\S]+?-----END CERTIFICATE-----/;
const KEY = /-----BEGIN (?:[A-Z]+ )?PRIVATE KEY-----[\s\S]+?-----END (?:[A-Z]+ )?PRIVATE KEY-----/;

/** What the certificate's name must be, as the page checks it before the server does. */
const NAME = /^(?!.*\.\.)[a-z0-9][a-z0-9.-]*$/;

/** What is wrong with the name `name` of a certificate, or ''. */
export function nameProblem(name) {
  if (name === '') return 'Give the certificate a name, such as the domain it is for.';
  if (name.length > 64 || !NAME.test(name)) {
    return 'A certificate\'s name has 1 to 64 lowercase letters, digits, hyphens and dots, starts with a letter or a digit, and has no two dots in a row.';
  }
  return '';
}

/** The size in bytes of `text` as it is sent. */
const bytesOf = (text) => new TextEncoder().encode(text).length;

/**
 * What is wrong with what was given, or null: `{field, words}` where `field`
 * is `chain` or `key`. The chain is read for certificates and must hold no
 * key; the key for one private key; neither has a NUL; together they fit.
 */
export function pemProblem(chain, key) {
  if (!CERTIFICATE.test(chain)) return { field: 'chain', words: 'This does not hold a certificate: it starts with -----BEGIN CERTIFICATE-----. Give the server\'s certificate first, then the intermediates.' };
  if (KEY.test(chain)) return { field: 'chain', words: 'This holds a private key. Give the key in the other box and only certificates here.' };
  if (!KEY.test(key)) return { field: 'key', words: 'This does not hold a private key: it has a -----BEGIN PRIVATE KEY----- block, or one for RSA or EC keys.' };
  if (CERTIFICATE.test(key)) return { field: 'key', words: 'This holds a certificate. Give only the private key here.' };
  if (chain.includes('\0') || key.includes('\0')) return { field: 'chain', words: 'A certificate is text: this has a byte that is not.' };
  if (bytesOf(`${key}\n${chain}\n`) > PEM_MAX) return { field: 'chain', words: 'The key and the chain together are 64 KiB at most.' };
  return null;
}

/**
 * The one document the action takes, the key first and the chain after,
 * with the line ends as `cert add` reads them. It holds the key: the caller
 * sends it once and lets go of it.
 */
export const documentOf = (chain, key) => `${key.replace(/\r\n?/g, '\n').trim()}\n${chain.replace(/\r\n?/g, '\n').trim()}\n`;

/** The arguments of `certificate.add` for the Show command, which never has the document: the server shows it as a secret. */
export function shownArgs({ name, replace, untrusted }) {
  return { name, ...(replace ? { replace: true } : {}), ...(untrusted ? { untrusted: true } : {}) };
}

/** The arguments of `certificate.add`, with the document `pem`. */
export const addArgs = (shown, pem) => ({ ...shownArgs(shown), pem });
