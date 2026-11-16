// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the This browser card says before anything is pressed, as plain values:
// whether the browser can get push, the note of what leaves the machine, and
// whether the machine lists this browser's subscription. The steps are in
// push-flow-model.js, which is read when they are taken.

/** The key `localStorage` keeps this browser's subscription's ID under. */
export const PUSH_ID_KEY = 'alerts.push.id';

/** What is said when the browser has no push. */
export const UNSUPPORTED_WORDS = 'This browser cannot get push notices.';

/** What is said of what leaves this machine when push is on. */
export const PRIVACY_WORDS = 'A push notice goes from this server to the push service of your browser\'s maker (for Chromium-based browsers, Google\'s; for Firefox, Mozilla\'s), which hands it to the browser. The notice\'s words are encrypted on this server for this browser alone (RFC 8291): the push service cannot read them. The push service does see this server\'s IP address, when each notice is sent and how big it is, the address it gave your browser (which it can tie to that browser), and this server\'s push key, which is the same on every notice, with the project\'s address as its contact, not yours. Nothing is sent until you turn push on in a browser, and Turn off ends it.';

/** Whether this browser can get push: it has service workers and the push manager. */
export const canPush = (env) => Boolean(env.navigator && 'serviceWorker' in env.navigator && 'PushManager' in env);

/** What this browser's card shows: `on` when the machine lists the subscription this browser kept, else `off`. `channels` are alertd's. */
export const hereState = (channels, id) => (id !== null && channels.some((one) => one.kind === 'push' && one.id === id) ? 'on' : 'off');
