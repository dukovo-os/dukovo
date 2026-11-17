// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What Restore… on a database's page says and sends, as plain values: the
// copies `databases.copies` lists and where each is, the name a restore into
// a new database gets, the arguments of `databases.restore`, the steps that
// run it, the words that say it is done and the open sessions it may stop
// for. restore.js draws them; it is read when Restore… is pressed.

import { nameProblem } from '../../lib/database-model.js';
import { bytesWords } from '../../lib/size-words.js';
import { partsOf } from '../../lib/time.js';

const isObject = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);

/**
 * The copies in `databases.copies`'s answer, newest first: `{atMs, bytes,
 * from, path}`. `from` is `live` for the folder in use, else the name of
 * the snapshot that holds the copy.
 */
export function copiesOf(answer) {
  const list = Array.isArray(answer?.copies) ? answer.copies : [];
  return list
    .filter((one) => isObject(one) && Number.isInteger(one.at_ms) && typeof one.path === 'string' && one.path !== '' && typeof one.from === 'string')
    .map((one) => ({
      atMs: one.at_ms, bytes: Number.isInteger(one.bytes) && one.bytes >= 0 ? one.bytes : 0, from: one.from, path: one.path,
    }))
    .sort((one, other) => other.atMs - one.atMs);
}

/** Where `copy` is: "This machine" for the folder in use, else the snapshot by its name. */
export const whereWords = (copy) => (copy.from === 'live' ? 'This machine' : `Snapshot ${copy.from}`);

/** The time of `copy` as a list shows it: the clock with "Today" before it, else the day and the clock. `time` is lib/time.js's. */
export function copyTime(copy, time) {
  const when = time.when(copy.atMs / 1000);
  // `when` writes the clock alone for a time of today.
  return /^\d/.test(when) ? `Today ${when}` : when;
}

/** What the dialog says when the database has no copy to put back. */
export const noCopyWords = (name) => `There is no copy of ${name} to put back. Copies are made before each backup.`;

/** The day of the Unix time `ms` in `zone` as a name writes it: 20261116. */
function dayOf(ms, zone) {
  const { year, month, day } = partsOf(zone, ms);
  return `${year}${String(month).padStart(2, '0')}${String(day).padStart(2, '0')}`;
}

/**
 * The name a restore of `name` into a new database gets unless another is
 * typed, as `database restore` makes it: `shop_restored_20261116`, with `_2`,
 * `_3` and on after it when one of `taken` has it. Empty text when no name
 * of the rule fits, which `name` being long does.
 */
export function restoredName(name, ms, zone, taken) {
  const base = `${name}_restored_${dayOf(ms, zone)}`;
  for (let number = 1; number < 100; number += 1) {
    const wanted = number === 1 ? base : `${base}_${number}`;
    if (nameProblem(wanted) !== '') return '';
    if (!taken.has(wanted)) return wanted;
  }
  return '';
}

/** Whether `name` can be kept under another name, as `shop_before_20261116_0200`, by the rule for names. */
export const keepFits = (name) => nameProblem(`${name}_before_20261116_0200`) === '';

/** Why `wanted` cannot be the new database's name, in words; empty text when it can. `taken` holds the databases there are. */
export const newNameProblem = (wanted, taken) => nameProblem(wanted) || (taken.has(wanted) ? `A database named ${wanted} is there already.` : '');

/**
 * The arguments of `databases.restore` for the copy at `path`: into the
 * new database `as`, or over the database itself with `over`, and `force`
 * to end the sessions open on it. A flag is there only when it is on.
 */
export function restoreArgs(name, {
  path, as = '', over = false, force = false,
}) {
  const args = { name, from: path };
  if (!over) {
    args.as = as;
    return args;
  }
  args.over = true;
  if (force) args.force = true;
  return args;
}

/** The step that runs a restore: `choice` is what the dialog gave. */
export const restoreStep = (name, choice) => ({
  title: choice.over ? `Restore ${name} over itself` : `Restore ${name} into ${choice.as}`,
  action: 'databases.restore',
  args: restoreArgs(name, choice),
  wait: 'task',
  words: {
    unlock: `restore ${name} from a copy`,
    running: `Restoring ${name}`,
    done: 'The copy is restored.',
    failed: 'The copy was not restored',
    stands: `${name} is as it was. What the restore made is taken back; if something could not be, the message says which database stays.`,
  },
});

/**
 * The words of the restore that has ended, as a list of text and `{name}`
 * for a database whose page the words link to. `kept` is the name the
 * database as it was is kept under, or null when it is not known.
 */
export function doneParts(name, choice, kept) {
  if (!choice.over) return ['Restored into ', { name: choice.as }];
  if (kept === null) return [`Restored into ${name}; the database before is kept under a name that starts with ${name}_before_: the list of databases has it.`];
  return [`Restored into ${name}; the database before is kept as `, { name: kept }];
}

/**
 * The database the restore over `name` kept: the one in `after` that is not
 * in `before` and is named as `database restore` names it, or null. Both are
 * the names of the databases as they were read.
 */
export function keptName(name, before, after) {
  const kept = new RegExp(`^${name}_before_\\d{8}_\\d{4}$`);
  return after.find((one) => !before.includes(one) && kept.test(one)) ?? null;
}

/**
 * The words that say a restore stopped for open sessions (DB-103): the last
 * line of the task `task` without the ID, which names the programs; null
 * when the task ended for another reason.
 */
export function openSessions(task) {
  const lines = Array.isArray(task?.log) ? task.log : [];
  const last = [...lines].reverse().find((line) => String(line).trim() !== '')?.trim() ?? '';
  return /\(DB-103\)$/.test(last) ? last.replace(/\s*\(DB-103\)$/, '') : null;
}

/** What the question that follows says: the restore keeps the database under another name, which needs no one connected. */
export const endSessionsWords = (name) => `The restore keeps ${name} under another name, and PostgreSQL does that only when no program is connected to it. Ending the sessions stops those programs at once, and what they were doing is lost.`;
