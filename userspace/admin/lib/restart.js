// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Restarting and shutting down the machine from a page: Restart now with the
// question that says what it ends, a restart at a time, Cancel, and Shut
// down with the machine's name typed. Each runs the action of the `power`
// area, which Spark carries out a minute later, and then hands the minute to
// the page's pending line (lib/pending-restart.js), which tells the shell
// that the server restarts when the minute ends. A page calls these when a
// button is pressed, so this file is read then. A restart that admind
// refuses, with a trial under way or a search for a bad change running, is
// said in the dialog in admind's own words.

import { h } from '../dom.js';
import { askToUnlock, isUnlocked } from '../unlock.js';
import { openFormDialog } from './form-dialog.js';
import { pendingRestart } from './pending-restart.js';
import {
  isClockTime, PEOPLE_MAX, restartLead, restartTitle, shutdownLead, shutdownTitle, signedInWords, taskWords, typedLabel, typedWhy,
} from './restart-dialog-model.js';

/** Whether changes may be made now: the console is unlocked, or the person unlocks it. `words` finish "You are about to". */
const unlocked = async (view, words) => isUnlocked() || askToUnlock(view.session, words);

/** The tasks that run now, which a restart ends. */
async function runningTasks(view) {
  const answer = await view.api.get('/api/1/tasks').catch(() => null);
  return (Array.isArray(answer?.tasks) ? answer.tasks : []).filter((task) => task.state === 'running' || task.state === 'waiting');
}

/**
 * Who is signed in with something that a restart ends: each account that may
 * use the console, with the browsers and terminals it has. An account whose
 * sessions cannot be read is left out.
 */
async function signedIn(view) {
  const answer = await view.api.get('/api/1/accounts').catch(() => null);
  const accounts = (Array.isArray(answer) ? answer : answer?.accounts ?? []).filter((one) => one.console === true).slice(0, PEOPLE_MAX);
  const people = await Promise.all(accounts.map(async (one) => {
    const found = await view.api.get(`/api/1/accounts/${encodeURIComponent(one.name)}/sessions`).catch(() => null);
    const browsers = Array.isArray(found?.sessions) ? found.sessions : [];
    return { name: one.name, sessions: browsers.filter((session) => session.this !== true).length, terminals: Array.isArray(found?.terminals) ? found.terminals.length : 0 };
  }));
  return signedInWords(people, view.session.name) ?? [];
}

/** The pending line the page gave, or one of this call's own, at the top of the page. */
function lineFor(view, pending) {
  if (pending) return pending;
  const own = pendingRestart(view);
  view.slot.append(own.element);
  return own;
}

/**
 * Asks, and restarts the machine in a minute. `why` is a sentence that says
 * what the restart is for ("To finish the update to 1.0.0-alpha.2."), `kind`
 * and `version` shape the banner's words (`update` with the version it
 * brings, or the one it goes back to), and `pending` is the page's own line
 * from pendingRestart, which counts the minute. Gives whether the restart was
 * set.
 */
export async function restartNow(view, {
  why = '', kind = 'machine', version = null, pending = null,
} = {}) {
  if (!(await unlocked(view, 'restart this machine'))) return false;
  const [people, tasks] = await Promise.all([signedIn(view), runningTasks(view)]);
  if (view.signal.aborted) return false;
  const call = { action: 'power.restart', args: { at: null } };
  const done = await openFormDialog(view, {
    title: restartTitle(view.host),
    icon: 'power',
    lead: [restartLead(view.host, why), ...people, ...tasks.map(taskWords)],
    label: 'Restart in a minute',
    command: call,
    submit: async (button, slot) => (await view.api.run(call.action, call.args, {
      button, where: { slot }, words: { unlock: 'restart this machine', done: `${view.host} restarts in a minute.` },
    })).ok,
  });
  if (!done) return false;
  const line = lineFor(view, pending);
  line.expect({ kind, version });
  await line.read();
  return true;
}

/** Sets a restart for `time`, `HH:MM` by the machine's clock, the next time it comes. Gives whether it was set. */
export async function restartAt(view, time, { kind = 'machine', version = null, pending = null } = {}) {
  if (!isClockTime(time)) {
    view.notice('A time is written HH:MM, from 00:00 to 23:59, such as 03:00. Nothing was set.', { kind: 'warn' });
    return false;
  }
  const result = await view.api.run('power.restart', { at: time }, {
    words: { unlock: `restart at ${time}`, done: `${view.host} restarts at ${time}.` },
  });
  if (!result.ok) return false;
  const line = lineFor(view, pending);
  line.expect({ kind, version });
  await line.read();
  return true;
}

/** Cancels the restart or shut down that is set. `button` is the one that was pressed. Gives whether it ran. */
export async function cancelRestart(view, { button = null } = {}) {
  const result = await view.api.run('power.cancel', {}, {
    button, words: { unlock: 'cancel the restart', done: 'It is cancelled.', unchanged: 'Nothing was set to cancel.' },
  });
  return result.ok;
}

/**
 * Asks for the machine's name to be typed, and shuts the machine down in a
 * minute. A shut down is never a restart: nothing brings the machine back
 * but its power button, and the dialog says so first. Gives whether it was
 * set.
 */
export async function shutDown(view, { pending = null } = {}) {
  if (!(await unlocked(view, 'shut down this machine'))) return false;
  const call = { action: 'power.shutdown', args: {} };
  const done = await openFormDialog(view, {
    title: shutdownTitle(view.host),
    icon: 'power',
    lead: [shutdownLead(view.host), h('p', {}, 'It shuts down in one minute, and you can cancel until then.')],
    label: 'Shut down in a minute',
    danger: true,
    typed: { name: view.host, label: typedLabel(view.host), why: typedWhy(view.host) },
    command: call,
    submit: async (button, slot) => (await view.api.run(call.action, call.args, {
      button, where: { slot }, words: { unlock: 'shut down this machine', done: `${view.host} shuts down in a minute.` },
    })).ok,
  });
  if (!done) return false;
  const line = lineFor(view, pending);
  line.expect({ kind: 'shutdown' });
  await line.read();
  return true;
}
