// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The steps that pages register for Finish setting up and /setup (design
// 3.5, 12.2), read from their parts and made into plain values: each part's
// `steps` are asked whether they are on this machine, whether they are done
// and what to say of them, in the 5 seconds a part has.

/**
 * The steps of every part that has some, in the order they say: `[{id,
 * title, order, points, action, done, summary, form}]`. A step whose
 * `available(cx)` says no is left out, and a part whose steps could not be
 * read is left out by the shell, with a note in the browser's console.
 */
export async function readSteps(view) {
  const found = await view.parts('overview', async (part, cx) => {
    const list = Array.isArray(part.steps) ? part.steps : [];
    return (await Promise.all(list.map(async (step) => {
      if (typeof step.available === 'function' && !(await step.available(cx))) return null;
      return {
        id: step.id,
        title: step.title,
        order: Number.isFinite(step.order) ? step.order : 1000,
        points: Number.isFinite(step.points) ? step.points : 0,
        action: step.action ?? null,
        done: Boolean(await step.done(cx)),
        summary: (await step.summary?.(cx)) ?? '',
        form: step.form ?? null,
      };
    }))).filter(Boolean);
  });
  return found.flatMap((one) => one.value).sort((a, b) => a.order - b.order || a.id.localeCompare(b.id));
}

/** "5 of 6 done", and the share done as a whole number of percent. */
export function progressOf(steps) {
  const done = steps.filter((step) => step.done).length;
  return { done, total: steps.length, words: `${done} of ${steps.length} done`, percent: steps.length === 0 ? 0 : Math.round((done * 100) / steps.length) };
}

/** The first step that is not done, or null. */
export const nextStep = (steps) => steps.find((step) => !step.done) ?? null;

const NUMBERS = ['No', 'One', 'Two', 'Three', 'Four', 'Five', 'Six', 'Seven', 'Eight', 'Nine', 'Ten'];

/** "Five steps" or "One step": a count of steps in words up to ten. */
export function stepsWords(count) {
  return `${NUMBERS[count] ?? String(count)} step${count === 1 ? '' : 's'}`;
}
