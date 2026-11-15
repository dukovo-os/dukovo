// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the first sign-in says and decides (design 12.1), as plain values:
// where it ends, which steps there are, the words of the time-zone step and
// the offer to set the machine up.

import { THEME_CHOICES } from '../../shell/prefs-model.js';
import { stepsWords } from '../overview/steps-read.js';

const WELCOME = '/welcome';

/**
 * The page that was asked for before /welcome, from its `then` address: a
 * path of the console, or `/`. Anything that could leave the console (a
 * scheme, `//`, a backslash or a control character, which a browser
 * reads as a slash) or come back to /welcome is `/`.
 */
export function thenPath(raw) {
  if (typeof raw !== 'string' || !raw.startsWith('/') || raw.startsWith('//')) return '/';
  if (/[\\\u0000-\u001f\u007f]/.test(raw)) return '/';
  const path = raw.split(/[?#]/)[0];
  if (path === WELCOME || path.startsWith(`${WELCOME}/`)) return '/';
  return raw;
}

/** The steps, in order: the time zone and the display, and the second step when the account has none. */
export function stepsOf({ second }) {
  return [
    { id: 'zone', title: 'Time zone' },
    { id: 'display', title: 'Display' },
    ...(second ? [{ id: 'second', title: 'Second step' }] : []),
  ];
}

/** What the time-zone step says is the zone: the browser's, with where it comes from, or the one that was chosen. */
export function zoneWords({ zone, browser }) {
  return zone === browser
    ? `Times in the console are shown in ${zone}, your browser's time zone.`
    : `Times in the console are shown in ${zone}.`;
}

/** What the server's clock is in, from `console.about`, or null when it does not say. */
export function serverClockWords(about) {
  return typeof about?.zone === 'string' && about.zone !== '' ? `The server's clock is in ${about.zone}.` : null;
}

/** The button that takes the server's zone in one click, after the zone that was asked for is refused. */
export const useServerWords = (zone) => `Use the server's: ${zone}`;

/** Whether `error` is the server saying it does not know the zone that was posted. */
export const isUnknownZone = (error) => error?.id === 'ADM-125' && error?.field === 'zone';

/** The step's line in the list at the left, from what has been chosen: `{zone, display: {theme, text, motion}, second}`. */
export function summaryOf(id, chosen) {
  if (id === 'zone') return chosen.zone ?? '';
  if (id === 'display') {
    const { theme, text, motion } = chosen.display ?? {};
    const label = THEME_CHOICES.find((choice) => choice.value === theme)?.label;
    return [label, text && `${text}% text`, motion === 'less' ? 'less motion' : null].filter(Boolean).join(', ');
  }
  if (id === 'second') return chosen.second ? 'Added' : 'Not added yet';
  return '';
}

/** The offer that ends an administrator's first sign-in: `{title, detail}`. */
export function offerWords(host, count) {
  return { title: `Set up ${host} now?`, detail: `${stepsWords(count)}, a few minutes each.` };
}
