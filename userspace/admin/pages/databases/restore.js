// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Restore… on a database's page: the copies that backups made of it, the
// question of where to put one (into a new database, or over the database,
// which is kept), the restore as a task with its steps, and the words that
// say where it went. Sessions open on the database stop an over until the
// person says they may be ended. Read when Restore… is pressed.

import { fill, h } from '../../dom.js';
import { hrefOf } from '../../lib/database-route.js';
import { openFormDialog } from '../../lib/form-dialog.js';
import { bytesWords, counted } from '../../lib/size-words.js';
import { ask } from '../../shell/dialogs.js';
import { banner } from './cards.js';
import { cardsField, textField } from './fields.js';
import {
  copiesOf, copyTime, doneParts, endSessionsWords, keepFits, keptName, newNameProblem, noCopyWords, openSessions, restoredName,
  restoreArgs, restoreStep, whereWords,
} from './restore-model.js';

/** The radio rows of the copies: the newest is chosen. Gives `{element, chosen()}`. */
function copyChoices(copies, time) {
  const rows = copies.map((copy, at) => {
    const input = h('input', { type: 'radio', name: 'restore-copy', value: copy.path, checked: at === 0 });
    const label = h('label', { class: 'radio-card' }, input,
      h('span', {}, h('b', {}, copyTime(copy, time)), h('small', { class: 'muted' }, `${bytesWords(copy.bytes)} · ${whereWords(copy)}`)));
    return { copy, input, label };
  });
  return {
    element: h('fieldset', { class: 'choices db-cards' }, h('legend', {}, 'Which copy'), rows.map((row) => row.label)),
    chosen: () => (rows.find((row) => row.input.checked) ?? rows[0]).copy,
  };
}

/**
 * Asks which of `copies` to put back and where: into a new database, or over
 * `db` (a row of the read), whose sessions are asked about only if they stop
 * it. `taken` holds the names of the databases there are. Gives `{path, as,
 * over}` (`restoreArgs`' choice), or null when the person cancelled; the page
 * runs the restore, which shows its steps.
 */
async function askRestore(view, {
  db, copies, taken,
}) {
  const { name } = db;
  const which = copyChoices(copies, view.time);
  const newName = textField({
    label: 'Name of the new database',
    value: restoredName(name, Date.now(), view.time.zone, taken),
    hint: `It has the owner and the sorting of ${name}, and ${name} is not changed.`,
  });
  const sessions = db.sessions > 0 ? ` It has ${counted(db.sessions, 'open session')}: you are asked before they are ended.` : '';
  const way = cardsField({
    legend: 'Where to put it',
    value: 'new',
    choices: [
      { value: 'new', title: 'As a new database', words: 'Nothing is written over.', sub: h('div', { class: 'db-sub' }, newName.element) },
      ...(keepFits(name) ? [{
        value: 'over', title: `Over ${name}`, words: `The database as it is now is kept as ${name}_before_… and the day and time.${sessions}`,
      }] : []),
    ],
  });
  let chosen = null;
  const choiceNow = () => {
    const copy = which.chosen();
    if (way.value() === 'over') return { path: copy.path, over: true };
    return { path: copy.path, as: newName.input.value.trim() };
  };
  const done = await openFormDialog(view, {
    title: `Restore ${name}`,
    icon: 'db',
    lead: [`Puts a copy of ${name} back. Whatever is there now stays as it is, in the database or under another name.`],
    fields: [which.element, way.element],
    label: 'Restore',
    command: () => {
      const now = choiceNow();
      return now.over || newNameProblem(now.as, taken) === '' ? { action: 'databases.restore', args: restoreArgs(name, now) } : null;
    },
    async submit() {
      const now = choiceNow();
      const why = now.over ? '' : newNameProblem(now.as, taken);
      if (why) newName.fault(why);
      else chosen = now;
      return chosen !== null;
    },
  });
  return done ? chosen : null;
}

/** Asks whether to end the sessions that stopped an over, which `words` names; gives whether to. */
async function askEnd(name, words) {
  const choice = await ask({
    title: `End the sessions of ${name}?`,
    words: [words, endSessionsWords(name)],
    iconName: 'warn',
    choices: [
      { label: 'Keep them', value: 'keep', kind: 'default', safe: true },
      { label: 'End the sessions and restore', value: 'end', kind: 'danger' },
    ],
  });
  return choice === 'end';
}

/** What a finished restore says, with links to the databases it names. */
const doneBanner = (parts) => banner('ok', 'check', parts.map((part) => (typeof part === 'string' ? part : h('a', { href: hrefOf(part.name) }, part.name))));

/**
 * Restores a copy of `db`: asks for the copy, runs the restore with
 * `flow(title, steps)` (the page's), and says where it went in `slot`. `read`
 * is the page's read of the databases and `errors` where its problems go.
 */
export async function restoreDatabase(view, {
  db, read, flow, errors, slot,
}) {
  const { name } = db;
  fill(errors);
  const asked = await view.api.run('databases.copies', { name }, { words: { unlock: `list the copies of ${name}` }, where: { slot: errors } });
  if (!asked.ok || view.signal.aborted) return;
  const copies = copiesOf(asked.result);
  if (copies.length === 0) {
    view.notice(noCopyWords(name), { kind: 'warn' });
    return;
  }
  const before = read.data.databases.map((one) => one.name);
  let choice = await askRestore(view, { db, copies, taken: new Set(before) });
  if (choice === null || view.signal.aborted) return;
  const title = `Restoring ${name}`;
  let outcome = await flow(title, [restoreStep(name, choice)]);
  const stopped = outcome.failed === 0 ? openSessions(outcome.answers[0]) : null;
  if (stopped !== null && choice.over && await askEnd(name, stopped) && !view.signal.aborted) {
    choice = { ...choice, force: true };
    outcome = await flow(title, [restoreStep(name, choice)]);
  }
  if (!outcome.done || view.signal.aborted) return;
  const kept = choice.over ? keptName(name, before, read.data?.databases.map((one) => one.name) ?? []) : null;
  slot.querySelector('section')?.append(doneBanner(doneParts(name, choice, kept)));
}
