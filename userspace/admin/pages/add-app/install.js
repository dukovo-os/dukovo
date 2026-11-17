// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Install for everyone: the one step that lib/steps.js runs, pinned by the
// SHA-256 of the sheet that was read, and what is read when it ends. What the
// page believes of the end comes from the machine and not from the runner: an
// app is installed when `bundle info` says that its version is the sheet's.

import { isStale } from '../../lib/sheet-model.js';

/**
 * How the steps are run: lib/steps.js's `runSteps(view, steps)`, which gives
 * `{element, finished}`, read when the first install is pressed, so that a
 * page that only looks does not carry it. A test puts its own function here.
 */
export const runner = {
  steps: async (view, steps) => (await import('../../lib/steps.js')).runSteps(view, steps),
};

/** The words of the install, as lib/steps.js takes them: the unlock dialog's, the notice's, and the title of the error when it fails. */
export const installWords = (name) => ({
  unlock: `install ${name} for everyone`, running: `Installing ${name}`, done: `${name} is installed.`, failed: `${name} was not installed`,
});

/** The step that installs the app of `sheet` (sheet-model's `sheetOf`) for everyone, from the very file whose sheet was read. */
export const installStep = (sheet) => ({
  title: `Install ${sheet.name} ${sheet.version} for everyone`,
  action: 'apps.install',
  args: { id: sheet.id, sha256: sheet.sha256 },
  wait: 'task',
  words: installWords(sheet.name),
});

/** `promise`'s answer, or null when it could not be had; a read that was cancelled is not an answer, and is raised. */
export const soft = (promise) => promise.catch((error) => {
  if (error?.name === 'AbortError') throw error;
  return null;
});

/**
 * Runs the install of `sheet` with the steps drawn into `box`, and says how it
 * ended: `{ok: true, info}` with `bundle info` of the app when it is installed
 * at the sheet's version, else `{ok: false, stale, left}`: `stale` is true when
 * the task's output says that the catalog offered another file than the
 * sheet's, `left` when the page was left before the end. The runner draws the
 * failure itself.
 */
export async function runInstall(view, sheet, box) {
  const run = await runner.steps(view, [installStep(sheet)]);
  box.append(run.element);
  const outcome = await run.finished;
  if (outcome.left) return { ok: false, stale: false, left: true };
  const info = await soft(view.api.get(`/api/1/apps/${encodeURIComponent(sheet.id)}`));
  if (info?.version === sheet.version) return { ok: true, info };
  return { ok: false, stale: isStale(null, outcome.answers?.[0] ?? null), left: false };
}

/** Spark's list of services, or null when it could not be read. */
export const readServices = (view) => soft(view.api.get('/api/1/services'));

/** Turns on the service `name`, which also starts it, with the words of the Services page; the button is put off while it runs. */
export async function turnOn(view, button, name) {
  const { wordsFor } = await import('../services/detail-model.js');
  return view.api.run('service.enable', { name }, { button, words: wordsFor('service.enable', name) });
}
