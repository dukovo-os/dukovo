// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// When the Alerts page draws the phone's panel or the computer's card of
// steps. The page reads it on every visit; the steps are read when drawn.

/** The key `localStorage` keeps "Not now" under. */
export const PHONE_DONE_KEY = 'alerts.phone.done';

/**
 * Whether the page draws the phone's panel: the address asked for it
 * (`?phone=1`, no other value counts), it was not put away, and the person is
 * an administrator.
 */
export const wantsPanel = ({ query, done, admin }) => admin === true && query?.get?.('phone') === '1' && done !== true;

/** Whether the page draws the computer's card: not on a phone, which is the phone. */
export const wantsCard = ({ narrow, admin }) => admin === true && narrow !== true;
