// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The machine's name and domain as the card says them: what `hostnamectl
// status --json` gave, its words, and what the card says once the name is
// changed.

/** What the card says of the rules. */
export const RULES_WORDS = 'Letters, digits and -, none starting or ending a word with -; a word is 63 characters at most and the whole name 64.';

/** The machine as `machine.name` answers: `{name, first, domain, local}`, or null when it did not read. */
export function machineOf(answer) {
  if (typeof answer?.name !== 'string' || answer.name === '') return null;
  const first = typeof answer.first === 'string' ? answer.first : answer.name.split('.')[0];
  const domain = typeof answer.domain === 'string' && answer.domain !== '' ? answer.domain : null;
  return {
    name: answer.name, first, domain, local: typeof answer.local === 'string' ? answer.local : `${first}.local`,
  };
}

/** "Full name: duke-nas.example.org · On this network: duke-nas.local". */
export const machineWords = (machine) => `Full name: ${machine.name} · On this network: ${machine.local}`;

/** What the card says once the name is changed. */
export function renamedWords(name, fingerprint) {
  return fingerprint === ''
    ? `The machine is called ${name} now. Its certificate is made anew: look at it in the list.`
    : `The machine is called ${name} now. The new fingerprint: ${fingerprint}`;
}
