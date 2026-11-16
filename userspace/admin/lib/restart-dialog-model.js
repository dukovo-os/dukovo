// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the dialogs that restart or shut down the machine say, and what the
// countdown under them says, as plain values (lib/restart.js and
// lib/pending-restart.js draw them). The words of the banner while the
// server is away are restart-model.js's.

import { counted } from './size-words.js';

/** Whether `text` is a time of day as the `power.restart` action takes it, `HH:MM` from 00:00 to 23:59. */
export const isClockTime = (text) => /^([01][0-9]|2[0-3]):[0-5][0-9]$/.test(String(text));

/** The most accounts whose sessions are read for the dialog's list of who is signed in. */
export const PEOPLE_MAX = 20;

/** The question of the dialog that restarts, with the machine's name `host`. */
export const restartTitle = (host) => `Restart ${host}?`;

/**
 * What a restart does: it begins a minute after the answer, and can be
 * cancelled until then. `why` is what the page that asks adds first ("To
 * finish the update to 1.0.0-alpha.2."), or an empty text.
 */
export function restartLead(host, why = '') {
  const words = `${host} restarts in one minute, and you can cancel until then. The console, SSH and every service stop and start again by themselves; open terminals end.`;
  return why ? `${why} ${words}` : words;
}

/** The question of the dialog that shuts down. */
export const shutdownTitle = (host) => `Shut down ${host}?`;

/** What a shut down does, which no page can undo from a distance. */
export const shutdownLead = (host) => `${host} does not come back until someone presses its power button. If nobody can reach it, use Restart instead.`;

/** The label of the field that holds the name typed to confirm a shut down. */
export const typedLabel = (host) => `Type ${host} to shut it down`;

/** Why the button of the shut down dialog is off, until the name is typed. */
export const typedWhy = (host) => `The button stays off until ${host} is typed exactly as it is written.`;

/**
 * The sentence for each person who is signed in with something that the
 * restart ends, from `people`: `[{name, sessions, terminals}]`, the counts
 * of browsers and of terminals; for `me`, the name of the person who asks,
 * the browsers are those other than the one in use. A person with none is
 * left out; it gives null when nobody is left.
 */
export function signedInWords(people, me) {
  const lines = people.filter((one) => one.sessions + one.terminals > 0).map((one) => {
    const own = one.name === me;
    const parts = [one.sessions > 0 && counted(one.sessions, own ? 'other browser' : 'browser'), one.terminals > 0 && counted(one.terminals, 'terminal')].filter(Boolean);
    return `${own ? 'You are' : `${one.name} is`} signed in with ${parts.join(' and ')}.`;
  });
  return lines.length === 0 ? null : lines;
}

/** "Back up to a place for nas is running and stops.": a task that the restart ends. */
export function taskWords(task) {
  const subject = Object.values(task.args ?? {}).filter((value) => ['string', 'number'].includes(typeof value) && value !== '').slice(0, 2).join(', ');
  return `${task.title ?? task.action}${subject ? ` for ${subject}` : ''} is running and stops.`;
}

/** The seconds left of a shutdown read `readAt` milliseconds ago (`performance.now()` values), at the time `now`. */
export function secondsLeft(shutdown, readAt, now) {
  if (!shutdown || typeof shutdown.in_seconds !== 'number') return null;
  return Math.max(0, shutdown.in_seconds - (now - readAt) / 1000);
}

/** What the page says once, as the last minute begins: "duke-nas restarts in 1 minute". */
export function minuteWords(shutdown, host) {
  return `${host} ${shutdown.end === 'reboot' ? 'restarts' : 'shuts down'} in 1 minute.`;
}

/** What it says once, as the minute ends. */
export const nowWords = (shutdown) => (shutdown.end === 'reboot' ? 'Restarting now.' : 'Shutting down now.');

/** The full-page notice after a shut down: nothing answers again until the machine is turned on. */
export const shutDownNotice = (host) => `${host} is shutting down. It will not answer again until someone turns it on.`;
