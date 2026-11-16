// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the sign-in rules, the console's reach and the blocked addresses say
// (design 8.4, 8.5 and 8.7) as plain values. They are made from `admin
// settings --json` (design 11.9) and `firewall blocked --json` (11.11).
// Nothing here touches the page.

import { counted } from '../../lib/size-words.js';

// ---- the sign-in rules (design 8.4) ---------------------------------------------------------

/** The second step's rules, the weakest first, as admin.toml(5) bounds them. */
export const SECOND_STEPS = Object.freeze([
  { value: 'if-set', label: 'Whoever has added one', who: 'anyone who has added one' },
  { value: 'administrators', label: 'Administrators, and whoever has added one', who: 'administrators, and anyone who has added one' },
  { value: 'everyone', label: 'Everyone', who: 'everyone' },
]);

const stepOf = (value) => SECOND_STEPS.find((one) => one.value === value) ?? SECOND_STEPS[0];

/** The rules of `admin settings --json`'s `sign_in`, with the defaults for what it does not say. */
export function rulesOf(answer) {
  const rules = answer?.sign_in ?? {};
  return {
    second_step: stepOf(rules.second_step).value,
    session_hours: Number.isInteger(rules.session_hours) ? rules.session_hours : 12,
    idle_minutes: Number.isInteger(rules.idle_minutes) ? rules.idle_minutes : 30,
  };
}

/** The two lines of the card, as the design has them. */
export function rulesWords(rules) {
  return [
    `Who gives a second step: ${stepOf(rules.second_step).who}.`,
    `A session lasts ${counted(rules.session_hours, 'hour')} after its sign-in, and ends after ${counted(rules.idle_minutes, 'minute')} with no console page open.`,
  ];
}

/** The note under the lines: whose rules these are. */
export const RULES_NOTE = 'These are the web console\'s rules and its API\'s. SSH signs in by its own settings, below.';

// ---- who may reach the console (design 8.7) ------------------------------------------------------

/** The row of the console's reach: "Port 9090 · this network and WireGuard only". */
export function reachRow(console) {
  const reach = console?.reach === 'any' ? 'anyone who can reach this server' : 'this network and WireGuard only';
  return [Number.isInteger(console?.listen) ? `Port ${console.listen}` : null, reach].filter(Boolean).join(' · ');
}

/** Whether the console answers anyone who can reach it. */
export const isPublic = (console) => console?.reach === 'any';

// ---- the addresses blocked after wrong sign-ins (design 8.5) -----------------------------------------

/** A length of time in words: "10 minutes", "an hour", "a day". */
function spanWords(seconds) {
  if (seconds >= 86_400 && seconds % 86_400 === 0) return seconds === 86_400 ? 'a day' : counted(seconds / 86_400, 'day');
  if (seconds >= 3600 && seconds % 3600 === 0) return seconds === 3600 ? 'an hour' : counted(seconds / 3600, 'hour');
  if (seconds >= 60 && seconds % 60 === 0) return counted(seconds / 60, 'minute');
  return counted(seconds, 'second');
}

/** The rule in words, from `firewall blocked --json`'s settings; empty text when it does not give them. */
export function blockingWords(answer) {
  const { tries, within_s: within, first_s: first, longest_s: longest } = answer ?? {};
  if (![tries, within, first, longest].every(Number.isInteger)) return '';
  return `${tries} wrong ${tries === 1 ? 'try' : 'tries'} in ${spanWords(within)} block an address for ${spanWords(first)}; each new block of it doubles, up to ${spanWords(longest)}. An administrator's own address is never blocked while they are signed in.`;
}

/** The setting's file and line, for blocking that is off. */
export const BLOCKING_OFF = 'Remove the line on = false from /system/settings/blocking.toml. Blocking is on when the file says nothing of it.';

/** The never-block list as a line: "Never blocked: 192.168.1.0/24 and 10.8.0.0/24"; the words that join are those of a list. */
export function neverWords(never) {
  const list = Array.isArray(never) ? never : [];
  if (list.length === 0) return 'Never blocked: no network';
  const joined = list.length === 1 ? list[0] : `${list.slice(0, -1).join(', ')} and ${list[list.length - 1]}`;
  return `Never blocked: ${joined}`;
}

