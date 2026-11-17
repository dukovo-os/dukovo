// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The machine's first set-up (design 12.2), for administrators: the steps
// that pages register at the left, each with its summary and a mark when it
// is done, and at the right the form of the step shown, the command it runs,
// and Set up, Skip for now and Back to the Overview. The page knows no step:
// each page that has one gives it through its overview part, and a server
// whose pages have none has nothing to show here. A step whose form has
// `args: null` is done on a page of its own: its form's `href` is offered as
// "Continue on its page" in place of Set up. A form that holds a secret gives
// `shown()` for the command, which reads none of it, and `clear()`, which the
// page calls when the step is left.

import { busy, h, icon, uniqueId } from '../../dom.js';
import { assess } from '../../lib/health.js';
import { atMostEvery } from '../overview/refresh.js';
import { readSteps } from '../overview/steps-read.js';
import {
  chosenStep, introWords, moveOnWords, previewOf, runWords, startedWords, stepAfter,
} from './model.js';

/** The least time between two reads that a task or change event asks for. */
const EVENT_GAP_MS = 5000;

/** How long the box that shows the command waits after a field changes, so that typing is one call. */
const COMMAND_WAIT_MS = 400;

export async function show(view) {
  const host = view.host || 'this server';
  view.head({ title: `Make ${host} safe in a few minutes`, crumbs: [{ title: 'Overview', path: '/' }], commands: true });

  let steps = await readSteps(view);
  if (view.signal.aborted) return;
  const back = h('a', { href: '/' }, 'Back to the Overview');
  if (steps.length === 0) {
    view.main.append(
      h('p', { class: 'muted' }, 'This server has nothing to set up yet.'),
      h('div', { class: 'btns' }, back));
    return;
  }

  // The score that the steps move. Left out, with its preview, when the checks have not answered.
  const score = await view.api.get('/api/1/checks/last').then((result) => assess(result).score, () => null);
  if (view.signal.aborted) return;

  let current = chosenStep(steps, view.query.get('step'));
  let form = null;

  // ---- The steps at the left ----
  const list = h('ol', { class: 'steps', 'aria-label': 'Steps' });
  const preview = h('div', { class: 'preview' });
  function drawMarks() {
    list.replaceChildren(...steps.map((step, at) => {
      const dot = h('span', { class: 'dot' }, step.done ? [icon('check'), h('span', { class: 'sr-only' }, 'Done: ')] : String(at + 1));
      const button = h('button', { class: `step${step.done ? ' done' : ''}${step.id === current.id ? ' cur' : ''}`, type: 'button', 'aria-current': step.id === current.id ? 'step' : null },
        dot, h('span', {}, step.title, h('br'), h('small', { class: 'muted' }, step.summary)));
      button.addEventListener('click', () => select(step.id));
      return h('li', {}, button);
    }));
    const shown = previewOf(current, score);
    if (!shown) {
      preview.replaceChildren();
      return;
    }
    const ring = h('div', { class: 'score mini', role: 'img', 'aria-label': shown.label }, h('span', {}, String(shown.from)));
    ring.style.setProperty('--v', String(shown.from));
    preview.replaceChildren(ring, h('span', { class: 'small' }, `This step takes the score from ${shown.from} to `, h('b', {}, String(shown.to)), '.'));
  }

  // ---- The step shown at the right ----
  const formBox = h('div', { class: 'step-form' });
  const commandBox = h('div', { class: 'cmd-box' });
  const run = h('button', { class: 'btn default', type: 'button' });
  const onPage = h('a', { class: 'btn default', href: '/', hidden: true }, 'Continue on its page');
  const skip = h('button', { class: 'btn', type: 'button' });
  const status = h('span', { class: 'small', role: 'status' });
  let waiting = null;

  /** The command this step runs, as the Show command panel draws it, for the fields as they are now. */
  function drawCommand() {
    clearTimeout(waiting);
    if (!current.action || !form) {
      commandBox.replaceChildren();
      return;
    }
    let args;
    try {
      // A form that holds a secret says what to show in `shown`, which reads none of it.
      args = (form.shown ?? form.args)();
    } catch {
      // A field that does not give arguments yet: the box keeps what it shows.
      return;
    }
    const id = uniqueId('runs');
    commandBox.replaceChildren(
      h('h3', { id, class: 'runs' }, icon('code'), 'This step runs'),
      h('div', { role: 'group', 'aria-labelledby': id }, view.api.panel(current.action, args)));
  }

  function drawButtons() {
    const elsewhere = form !== null && form.args === null && typeof form.href === 'string';
    run.textContent = runWords(current);
    run.setAttribute('aria-label', `${runWords(current)}: ${current.title}`);
    run.hidden = !current.action || elsewhere;
    onPage.hidden = !elsewhere;
    if (elsewhere) onPage.setAttribute('href', form.href);
    onPage.setAttribute('aria-label', `Continue on its page: ${current.title}`);
    skip.textContent = moveOnWords(current);
    // A step that is done and has no other left to go to has nothing to move on to.
    skip.hidden = current.done && stepAfter(steps, current.id) === null;
  }

  /** Shows `id`; a press on a step moves focus to the step's heading, so that a screen reader says where it is. */
  function select(id, { focus = true } = {}) {
    current = steps.find((step) => step.id === id) ?? current;
    history.replaceState(null, '', `/setup?step=${encodeURIComponent(current.id)}`);
    status.textContent = '';
    form?.clear?.();
    form = null;
    formBox.replaceChildren();
    if (typeof current.form === 'function') {
      try {
        form = current.form(view);
      } catch {
        formBox.append(h('p', { class: 'muted', role: 'status' }, 'This step\'s form could not be shown. Reload the page.'));
      }
    }
    if (form) {
      formBox.append(form.element);
      form.element.addEventListener('change', () => {
        clearTimeout(waiting);
        waiting = setTimeout(drawCommand, COMMAND_WAIT_MS);
      });
    } else if (!formBox.firstChild) {
      formBox.append(h('section', { class: 'card' }, h('h2', { tabindex: '-1' }, current.title), current.summary && h('p', { class: 'muted' }, current.summary)));
    }
    drawMarks();
    drawButtons();
    drawCommand();
    const heading = formBox.querySelector('h2');
    heading?.setAttribute('tabindex', '-1');
    if (focus) heading?.focus();
  }

  /** Reads the steps again, and draws what changed without making the form again, so that what was typed stays. */
  async function reload() {
    const was = current;
    let read;
    try {
      read = await readSteps(view);
    } catch {
      return;
    }
    if (view.signal.aborted || read.length === 0) return;
    steps = read;
    current = steps.find((step) => step.id === was.id) ?? chosenStep(steps, null);
    if (current.id !== was.id) {
      select(current.id, { focus: false });
      return;
    }
    drawMarks();
    drawButtons();
  }

  run.addEventListener('click', async () => {
    let args;
    try {
      args = form ? form.args() : {};
    } catch (error) {
      // A form that names what is missing says so; one that has nothing to say leaves the status as it is.
      status.textContent = error?.words ?? '';
      return;
    }
    status.textContent = '';
    const result = await view.api.run(current.action, args, { button: run });
    if (result?.ok) {
      status.textContent = startedWords(result.result);
      // A step whose answer is shown once (a recovery key) shows it before the page moves on.
      try {
        await form?.after?.(result.result);
      } catch {
        // The step is done; what it answered (a recovery key) is shown again on the page of what it made.
        status.textContent = `${startedWords(result.result)} Its answer could not be shown here: open what it made to see it again.`;
      }
      if (!result.task) reload();
    }
  });

  skip.addEventListener('click', () => {
    const next = stepAfter(steps, current.id);
    if (next) select(next.id);
    else view.go('/');
  });

  // A task that ends or a change that is made may finish a step.
  const again = atMostEvery(reload, EVENT_GAP_MS);
  view.on('task', again);
  view.on('change', again);
  view.signal.addEventListener('abort', () => { again.stop(); clearTimeout(waiting); form?.clear?.(); }, { once: true });

  view.main.append(
    h('p', { class: 'muted' }, introWords(steps)),
    h('div', { class: 'wiz setup' },
      h('div', { class: 'steps-col card' }, list, preview),
      h('div', { class: 'form-col' },
        formBox,
        commandBox,
        h('div', { class: 'btns' }, run, onPage, skip, status, h('span', { class: 'spacer' }), back))));

  select(current.id, { focus: false });
}
