// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// How this server updates and the checks after an update, as plain values: the
// questions of the settings form, the arguments of `update set` that what a
// person changed makes and the arguments that put it back, the words of each
// check, and the arguments of a check that is added. No DOM.

import { clockWords } from './model.js';

/** A time of day as `update set --restart` takes it. */
export const TIME = /^([01][0-9]|2[0-3]):[0-5][0-9]$/;

/** The time offered when the system is first set to restart by itself. */
export const DEFAULT_RESTART = '03:00';

/** The questions: each has its choices as `[value, words]`, the machine's key and a note for the choice that needs one. */
export const QUESTIONS = Object.freeze([
  {
    key: 'check', legend: 'Look for updates', choices: [['notify', 'Every day, and tell me'], ['download', 'Every day, download and tell me'], ['off', 'Never']],
  },
  {
    key: 'system', legend: 'The system', choices: [['manual', 'When I say'], ['auto', 'By itself, and restart at']],
  },
  {
    key: 'apps', legend: 'Apps', choices: [['notify', 'Tell me'], ['auto', 'Update by themselves']],
  },
  {
    key: 'metered', legend: 'On a metered connection', choices: [['wait', 'Wait'], ['download', 'Download anyway']],
  },
  {
    key: 'channel', legend: 'Channel', choices: [['stable', 'Stable'], ['beta', 'Beta'], ['nightly', 'Nightly']],
  },
]);

/** The form's values from `update settings --json`. */
export function formOf(settings) {
  return {
    check: settings?.check ?? 'notify',
    system: settings?.unattended ? 'auto' : 'manual',
    restart: TIME.test(settings?.restart ?? '') ? settings.restart : DEFAULT_RESTART,
    apps: settings?.apps ?? 'notify',
    metered: settings?.metered ?? 'wait',
    channel: settings?.channel ?? 'stable',
  };
}

/**
 * The arguments that change `before` into `after` (both form values): `{set,
 * channel, problem}`. `set` is the arguments of `updates.set` for what
 * moved, or null when nothing of it did; `channel` is the new channel for
 * `updates.channel`, or null. A system set to restart by itself at a time
 * that is not `HH:MM` is a `problem`, in words, and nothing is changed.
 */
export function argsOf(before, after) {
  const set = {};
  for (const key of ['check', 'apps', 'metered']) if (before[key] !== after[key]) set[key] = after[key];
  if (after.system === 'auto' && !TIME.test(after.restart)) {
    return { set: null, channel: null, problem: 'A time is written HH:MM, from 00:00 to 23:59, such as 03:00.' };
  }
  if (before.system !== after.system) {
    set.unattended = after.system === 'auto';
    set.restart = after.system === 'auto' ? after.restart : null;
  } else if (after.system === 'auto' && before.restart !== after.restart) {
    set.restart = after.restart;
  }
  return {
    set: Object.keys(set).length > 0 ? set : null,
    channel: before.channel === after.channel ? null : after.channel,
    problem: null,
  };
}

const AGE = { stable: 0, beta: 1, nightly: 2 };

/** The note under a choice, or null: what Never costs, what By itself waits for, a channel that is older, bundled's setting that disagrees. */
export function noteOf(key, form, { disagree = false } = {}) {
  if (key === 'check' && form.check === 'off') return 'Nobody is told of security fixes. SEC-131 warns after 7 days.';
  if (key === 'system' && form.system === 'auto') return `Outside ${clockWords(TIME.test(form.restart) ? form.restart : DEFAULT_RESTART)} nothing restarts on its own; an update staged by itself waits for that time.`;
  if (key === 'apps' && disagree) return 'Apps are set to update by themselves for everyone in another place. Saving makes the two agree.';
  return null;
}

/** The note under Channel when the choice is an older channel than the one now: the system does not go back through a channel. */
export function channelNote(before, after, running) {
  if (before === after || AGE[after] >= AGE[before]) return null;
  const name = after.charAt(0).toUpperCase() + after.slice(1);
  return `This system does not go back to an older version through a channel: it waits until ${name} passes ${running ?? 'the version it runs'}.`;
}

// ---- the checks after an update ----------------------------------------------

/** What one check does, as the health check runs it: "https://localhost/ answers", "nginx is running". */
export function whatCheck(check) {
  if (check.kind === 'url' && check.target) return `${check.target} answers`;
  if (check.kind === 'service' && check.target) return `${check.target} is running`;
  return Array.isArray(check.command) && check.command.length > 0 ? `runs ${check.command.join(' ')}` : 'runs a command';
}

/**
 * The checks card from `update checks --json`: the services that must run
 * again, each check split into required and wanted with its words, and the
 * seen-through time and deadline in one sentence.
 */
export function checksCard(doc) {
  const rows = (Array.isArray(doc?.checks) ? doc.checks : []).filter((check) => typeof check?.name === 'string');
  const row = (check) => ({ name: check.name, line: `${check.name} · ${whatCheck(check)}${Number.isFinite(check.timeout) ? ` within ${check.timeout} s` : ''}` });
  return {
    essential: (Array.isArray(doc?.essential) ? doc.essential : []).filter((name) => typeof name === 'string'),
    required: rows.filter((check) => check.required).map(row),
    wanted: rows.filter((check) => !check.required).map(row),
    seen: Number.isFinite(doc?.stable) && Number.isFinite(doc?.deadline)
      ? `Seen through after ${doc.stable} seconds with no restart; an update has ${doc.deadline} seconds to start them all.` : null,
  };
}
