// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A QR code that hands a link to a phone (Report a problem's download, Alerts'
// Open on your phone): the code drawn by qr.js, the link as text under it
// that can be selected, the certificate's fingerprint in groups, the words
// that tell what the phone's warning means and a countdown. The page that
// shows it reads this file with `import()` when a code is wanted, so that
// qr.js is not part of the page's start. Nothing runs at import.

import { h } from '../dom.js';
import { qrSvg } from '../qr.js';
import {
  countdownWords, fingerprintGroups, pictureWords, WARNING_WORDS,
} from './qr-view-model.js';

/** The QR code of `text` for a screen reader named `label`, or null when `text` is longer than a code holds. */
export function qrView(text, label) {
  try {
    return qrSvg(text, label);
  } catch (error) {
    if (error instanceof RangeError) return null;
    throw error;
  }
}

/**
 * The picture of a link to hand to a phone: `{url, fingerprint, expires_ms}`
 * and what it opens (`what`: "report.txt"). Gives `{element, stop}`:
 * `stop()` ends the countdown, which also ends when the page's `signal`
 * aborts; `onEnded` is called once, when the time is up. `clock` gives the
 * time now in milliseconds; tests replace it.
 */
export function phoneLink({
  url, fingerprint, expires_ms: expires, what = 'report.txt', signal, onEnded = () => {}, clock = Date.now,
}) {
  const picture = qrView(url, pictureWords(what));
  const countdown = h('p', { role: 'timer', 'aria-live': 'off' }, countdownWords(expires - clock()));
  const groups = fingerprintGroups(fingerprint);
  const element = h('div', { class: 'qr-row' },
    picture ?? h('p', { class: 'note' }, 'The link is too long for a QR code.'),
    h('div', { class: 'field' },
      h('p', {}, WARNING_WORDS),
      h('span', { class: 'label' }, 'The link'),
      h('p', { class: 'cmdtext' }, url),
      groups.length > 0 && h('span', { class: 'label' }, 'The certificate\'s SHA-256 fingerprint'),
      groups.length > 0 && h('p', { class: 'cmdtext' }, groups.join(' ')),
      countdown));

  let ticker = null;
  const stop = () => {
    clearInterval(ticker);
    ticker = null;
  };
  const tick = () => {
    const left = expires - clock();
    countdown.textContent = countdownWords(left);
    if (left <= 0 && ticker !== null) {
      stop();
      onEnded();
    }
  };
  if (expires - clock() > 0) ticker = setInterval(tick, 1000);
  else queueMicrotask(onEnded);
  signal?.addEventListener('abort', stop, { once: true });
  return { element, stop };
}
