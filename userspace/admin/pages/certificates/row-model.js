// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the dialogs and the changes of a row say: why a certificate is
// revoked, what making this server's own anew does to browsers, how the
// console's certificate is tried, and what stopping the tries for a
// certificate that could not be got means. The page reads this file with the
// first button of a row that is pressed.

/** The reasons `cert revoke` takes, as the authority records them, and the words a person reads. */
export const REVOKE_REASONS = Object.freeze([
  { id: 'unspecified', words: 'No reason given' },
  { id: 'key-compromise', words: 'Its key may have been seen by someone else' },
  { id: 'superseded', words: 'A new certificate replaces it' },
  { id: 'cessation-of-operation', words: 'The site is gone' },
]);

/** What revoking does not do: the services that have the certificate keep using it. */
export const REVOKE_WORDS = 'Revoking tells the authority never to trust it again. Services keep using the certificate until a new one replaces it, and a revoked certificate cannot be undone.';

/** What making this server's own certificate anew does to browsers. */
export const MAKE_ANEW_WORDS = 'This server makes its own certificate again. Browsers and phones that trusted the old one warn again, and its new fingerprint is shown when it is made.';

/** What the dialog that stops certd trying to get `name` says. */
export const forgetWords = (name) => `Stop trying to get ${name}? certd will not ask the authority for it again, and its alert goes away.`;

/** How long a change of the certificate the console serves is tried for, in seconds. */
export const SERVE_TRY_SECONDS = 300;

/** The line of the banner while the console serves a certificate that is tried. */
export const servingWords = (name) => `This console now serves ${name}'s certificate. Keep it once this page has come back on it.`;
