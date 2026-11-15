// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The steps this folder gives to Finish setting up and to /setup (design
// 12.2): until C5 and C6 add the six real ones, a step that only a lab build
// has, which runs the lab's long action. A shipped build does not offer that
// action, so `available` is false there and the card and the page show no
// step at all.
//
// A step is `{id, title, order, points, action, available, done, summary,
// form}`: `available(cx)` says whether the step is on this machine (it is
// when left out), `done(cx)` whether it is finished, `summary(cx)` says
// where it is in words, and `form(view)` gives `{element, args}`, the fields
// of the step and a function that reads them as the action's arguments.

import { h, uniqueId } from '../../dom.js';

/** The action this step runs. */
export const LAB_ACTION = 'lab.long';

/** The list of actions, whether it is the array or an object that holds it. */
const actionsOf = (answer) => (Array.isArray(answer) ? answer : answer?.actions ?? []);

const offered = async (cx) => actionsOf(await cx.cache('/api/1/actions', 60_000, () => cx.api.get('/api/1/actions'))).some((one) => one.id === LAB_ACTION);

/** Whether the lab's long action has run to its end once. */
const ran = async (cx) => {
  const answer = await cx.cache('/api/1/tasks', 5000, () => cx.api.get('/api/1/tasks'));
  return (answer?.tasks ?? []).some((task) => task.action === LAB_ACTION && task.state === 'done');
};

export const steps = [{
  id: 'lab-long',
  title: 'Run the lab\'s long test',
  order: 1000,
  points: 0,
  action: LAB_ACTION,
  available: offered,
  done: ran,
  summary: async (cx) => ((await ran(cx)) ? 'It ran to its end' : 'It has not run yet'),
  form(view) {
    const id = uniqueId('lab-seconds');
    const input = h('input', { id, class: 'input', type: 'number', min: '1', max: '600', value: '3', inputmode: 'numeric', 'aria-describedby': `${id}-hint` });
    const element = h('section', { class: 'card', 'aria-labelledby': `${id}-h` },
      h('h2', { id: `${id}-h` }, 'Run the lab\'s long test'),
      h('p', { class: 'muted' }, 'A change that runs as a task for a few seconds and changes nothing on the machine. A lab build has it, to try the steps of this page.'),
      h('div', { class: 'field' },
        h('label', { for: id }, 'Seconds it runs'),
        input,
        h('span', { id: `${id}-hint`, class: 'hint' }, 'From 1 to 600.')));
    return { element, args: () => ({ seconds: Math.max(1, Math.min(600, Number(input.value) || 3)) }) };
  },
}];
