// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What /setup decides and says (design 12.2), as plain values: which step is
// shown, which comes after it, what a step does to the health score and the
// words above the wizard. The steps are the values steps-read.js gives:
// `{id, title, points, action, done, summary, form}`.

import { stepsWords } from '../overview/steps-read.js';

/** The step to show: the one the address names, else the first that is not done, else the first; null when there are no steps. */
export function chosenStep(steps, wanted) {
  return steps.find((step) => step.id === wanted) ?? steps.find((step) => !step.done) ?? steps[0] ?? null;
}

/** The step to go to from `id`: the next one that is not done, counting on from it and then from the start; null when no other step is left. */
export function stepAfter(steps, id) {
  const at = steps.findIndex((step) => step.id === id);
  const around = [...steps.slice(at + 1), ...steps.slice(0, Math.max(at, 0))];
  return around.find((step) => !step.done) ?? null;
}

/**
 * What `step` does to the score of `score` (null when the checks have not
 * answered): `{from, to, words, label}`, or null when it does nothing, as
 * for a step that is done, one that gives no points or a score that is full.
 */
export function previewOf(step, score) {
  if (step.done || !Number.isFinite(score) || !(step.points > 0) || score >= 100) return null;
  const to = Math.min(100, score + step.points);
  return {
    from: score,
    to,
    words: `This step takes the score from ${score} to ${to}.`,
    label: `Health score ${score}, ${to} after this step`,
  };
}

/** The line above the wizard: how many steps there are, and what finishing them later means. */
export function introWords(steps) {
  const raises = steps.some((step) => step.points > 0);
  return [
    `${stepsWords(steps.length)}. Each can be skipped and finished later from the Overview.`,
    raises && 'Each one raises the health score.',
  ].filter(Boolean).join(' ');
}

/** The button that runs a step: "Set up", or "Set up again" for one that is done. */
export const runWords = (step) => (step.done ? 'Set up again' : 'Set up');

/** The button that moves on: "Skip for now" for a step that is not done, "Next step" for one that is. */
export const moveOnWords = (step) => (step.done ? 'Next step' : 'Skip for now');

/** The words of what a run started, for the line under the buttons: a task, or a change that was done at once. */
export const startedWords = (answer) => (typeof answer?.task === 'number' ? 'Started. Its progress is in Tasks.' : 'Done.');
