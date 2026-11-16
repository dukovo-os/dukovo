// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the forms of the sign-in rules, the console's reach and the never-block
// list ask (design 8.4, 8.7 and 22.7) as plain values: the bounds of the
// numbers, what a change of the rules says before it is sent, what making the
// console public or private says, and what to do with a network that is to be
// never blocked. Nothing here touches the page.

import { formatNetwork, networkOf, parseNetwork } from '../../lib/address-model.js';
import { counted } from '../../lib/size-words.js';
import { isOutside } from './model.js';
import { SECOND_STEPS } from './rules-model.js';

/** The bounds of the session's hours and of the minutes with no console page open. */
export const HOURS = Object.freeze({ least: 1, most: 168 });
export const MINUTES = Object.freeze({ least: 5, most: 1440 });

/** What is wrong with a number of the form, as a sentence, or empty text. */
function boundProblem(text, { least, most }, noun) {
  const value = Number(text);
  if (String(text).trim() === '' || !Number.isInteger(value) || value < least || value > most) return `Write a whole number of ${noun} from ${least} to ${most}.`;
  return '';
}

/** The problems of the form `{second_step, hours, minutes}` by field; empty when it is fit to send. */
export function rulesProblems(form) {
  const found = {};
  const hours = boundProblem(form.hours, HOURS, 'hours');
  const minutes = boundProblem(form.minutes, MINUTES, 'minutes');
  if (hours) found.hours = hours;
  if (minutes) found.minutes = minutes;
  return found;
}

/**
 * What the form changes from `rules`: `{args, lines}`. `args` are the
 * arguments of `security.sign-in-set`, only what changed; `lines` say what
 * follows, before it is sent.
 */
export function ruleChange(rules, form) {
  const args = {};
  const lines = [];
  const rank = (value) => SECOND_STEPS.findIndex((one) => one.value === value);
  if (form.second_step !== rules.second_step) {
    args.second_step = form.second_step;
    if (rank(form.second_step) > rank(rules.second_step)) lines.push('Accounts that have none add one at their next sign-in, before anything else. Nobody is locked out.');
  }
  const hours = Number(form.hours);
  if (hours !== rules.session_hours) {
    args.session_hours = hours;
    if (hours < rules.session_hours) lines.push(`Sessions older than ${counted(hours, 'hour')} end within 30 seconds, this one too if it is: you sign in again.`);
  }
  const minutes = Number(form.minutes);
  if (minutes !== rules.idle_minutes) {
    args.idle_minutes = minutes;
    if (minutes < rules.idle_minutes) lines.push(`Sessions with no console page open for more than ${counted(minutes, 'minute')} end within 30 seconds.`);
  }
  return { args, lines };
}

/** What making the console public asks for, in a few words, by the second-step rule now. */
export function publicWords(rules) {
  return [
    'Anyone who can reach this server can then load the console\'s sign-in page. admin.toml(5) asks for care: give administrators a second step first.',
    rules.second_step === 'if-set' ? 'The rule now asks for a second step only of those who have added one.' : null,
  ].filter(Boolean);
}

/** What a change to the network only says of a browser that comes from outside it (design 8.7); `ends` is when the change is undone, as the clock reads it. */
export const cutsWords = (from, ends) => `This browser comes from ${from}, which is outside. After this change it cannot reach the console: the change is undone at ${ends} unless it is kept from a browser on this network.`;

/** The network that is never blocked, widest to allow without asking: IPv4's /16 and IPv6's /48. */
const WIDEST = Object.freeze({ 4: 16, 6: 48 });

/**
 * What to do with the text of the never-block field (design 22.7): `{network,
 * problem, ask}`. `problem` says that it is not an address or a network;
 * `ask` is the question of a network that is not private or is wider than a
 * /16 (IPv4) or /48 (IPv6), whose answer is the person's.
 */
export function neverCheck(text) {
  const word = String(text ?? '').trim();
  const network = parseNetwork(word);
  if (network === null) return { network: word, problem: 'Write an address or a network, such as 192.168.1.0/24.', ask: null };
  const wide = network.prefix < WIDEST[network.address.version];
  const ask = isOutside(word) || wide
    ? `Addresses in ${formatNetwork(networkOf(network))} are never blocked, however often they get a password wrong.`
    : null;
  return { network: word, problem: '', ask };
}
