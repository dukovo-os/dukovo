// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The first sign-in of each person (design 12.1), in the board's wizard
// frame: the steps at the left, one at a time at the right, Back, Next and
// Skip for now. The time zone is shown first and is saved as the step is
// left, so that this page does not come back; the display previews each
// choice and saves at Next; the second step is the account's own page
// (account-steps.js) when the account has none. Nothing is asked twice: the
// browser's zone is the answer until it is changed.

import { ApiError } from '../../api.js';
import {
  busy, fill, h, icon, uniqueId,
} from '../../dom.js';
import { browserZone } from '../../lib/time.js';
import { displayOf } from '../../shell/prefs-model.js';
import { displayChoices, zoneField } from '../account/controls.js';
import { hasSecondStep } from '../account/needs.js';
import { readSteps, stepsWords } from '../overview/steps-read.js';
import {
  isUnknownZone, offerWords, serverClockWords, stepsOf, summaryOf, thenPath, useServerWords, zoneWords,
} from './model.js';

/** What a person is told who leaves the zone empty, in the form of the server's words for a zone it does not know, so that it lands under the field the same way. */
const EMPTY_ZONE = new ApiError({
  id: 'ADM-125',
  title: 'The time zone is empty',
  detail: 'The time zone is empty, and the console needs one to show times in.',
  fix: 'Type a place, such as Europe/Prague, and pick it from the list.',
  status: 400,
  field: 'zone',
});

/** What the answer of a read gives, or `fallback` when the read failed. */
const valueOf = (settled, fallback) => (settled.status === 'fulfilled' ? settled.value : fallback);

export async function show(view) {
  const host = view.host || 'this server';
  view.head({ title: `Welcome to ${host}` });

  const [aboutRead, secondRead] = await Promise.allSettled([
    view.api.get('/api/1/console/about'),
    view.api.get('/api/1/account/second-step'),
  ]);
  if (view.signal.aborted) return;
  const about = valueOf(aboutRead, null);
  // A read that failed is not a reason to offer a step: the Overview says so when it can read.
  const offerSecond = secondRead.status === 'fulfilled' && !hasSecondStep(secondRead.value);
  const steps = stepsOf({ second: offerSecond });

  const browser = browserZone();
  const serverZone = typeof about?.zone === 'string' && about.zone !== '' ? about.zone : null;
  const chosen = { zone: browser, display: displayOf(view.prefs), second: false };
  let zoneSaved = false;
  let index = 0;

  // A choice that is previewed and not saved goes back when the page is left.
  view.signal.addEventListener('abort', () => view.previewDisplay(null), { once: true });

  // ---- The steps at the left ----
  const marks = steps.map((step) => {
    const dot = h('span', { class: 'dot' });
    const summary = h('small', { class: 'muted' });
    const element = h('div', { class: 'step' }, dot, h('span', {}, step.title, h('br'), summary));
    return { id: step.id, element, dot, summary, done: false };
  });
  const drawMarks = () => {
    marks.forEach((mark, at) => {
      mark.element.classList.toggle('cur', at === index);
      mark.element.classList.toggle('done', mark.done);
      if (at === index) mark.element.setAttribute('aria-current', 'step');
      else mark.element.removeAttribute('aria-current');
      mark.summary.textContent = mark.done || at === index ? summaryOf(mark.id, chosen) : '';
      fill(mark.dot, mark.done ? [icon('check'), h('span', { class: 'sr-only' }, 'Done: ')] : String(at + 1));
    });
  };

  // ---- The time zone ----
  const zone = zoneField({ value: chosen.zone });
  const zoneNow = h('p', { id: uniqueId('zone-now') });
  const change = h('button', { class: 'btn small', type: 'button' }, 'Change');
  const useServer = h('button', { class: 'btn small', type: 'button', hidden: true }, serverZone ? useServerWords(serverZone) : '');
  const serverClock = serverClockWords(about);
  const zoneHeading = h('h2', { id: uniqueId('welcome-zone'), tabindex: '-1' }, 'Time zone');
  const zonePanel = h('section', { class: 'card panel', 'aria-labelledby': zoneHeading.id },
    zoneHeading,
    h('div', { class: 'zone-now' }, zoneNow, change),
    zone.field,
    useServer,
    serverClock && h('p', { class: 'muted' }, serverClock));
  zone.field.hidden = true;

  const drawZone = () => {
    zoneNow.textContent = zoneWords({ zone: chosen.zone, browser });
  };
  const showField = () => {
    zone.field.hidden = false;
    change.hidden = true;
  };
  change.addEventListener('click', () => {
    showField();
    zone.input.focus();
    zone.input.select();
  });
  zone.input.addEventListener('input', () => { useServer.hidden = true; });
  zone.input.addEventListener('keydown', (event) => {
    if (event.key === 'Enter') {
      event.preventDefault();
      next.click();
    }
  });
  // The zone that is on the screen: the browser's until the field is opened.
  const shownZone = () => (zone.field.hidden ? chosen.zone : zone.value());

  /** Saves `wanted` as the person's zone; gives whether the server kept it. A refusal is said under the field, with the server's own zone to take in one click. */
  async function saveZone(wanted) {
    if (wanted === '') {
      showField();
      view.problem(EMPTY_ZONE, { input: zone.input });
      return false;
    }
    try {
      await view.setPrefs({ zone: wanted });
    } catch (error) {
      if (isUnknownZone(error)) {
        showField();
        useServer.hidden = serverZone === null;
      }
      view.problem(error, { input: zone.input });
      return false;
    }
    zoneSaved = true;
    chosen.zone = wanted;
    drawZone();
    return true;
  }
  useServer.addEventListener('click', async () => {
    zone.input.value = serverZone;
    useServer.hidden = true;
    zone.input.dispatchEvent(new Event('input'));
    if (await busy([useServer, next], () => saveZone(serverZone))) {
      marks[0].done = true;
      go(1);
    }
  });

  // ---- The display ----
  const displayHeading = h('h2', { id: uniqueId('welcome-display'), tabindex: '-1' }, 'Display');
  const displayChoicesBox = h('div', {});
  const displayPanel = h('section', { class: 'card panel', 'aria-labelledby': displayHeading.id },
    displayHeading,
    h('p', { class: 'muted' }, 'Each choice shows at once. Next keeps them for every browser you sign in on.'),
    displayChoicesBox);
  const drawDisplay = () => {
    displayChoicesBox.replaceChildren(displayChoices({
      current: chosen.display,
      onChange: (choice) => {
        chosen.display = { ...chosen.display, ...choice };
        view.previewDisplay(chosen.display);
        drawMarks();
      },
    }));
  };

  // ---- The second step ----
  const secondPanel = offerSecond ? h('div', { class: 'panel second' }) : null;
  let secondDrawn = null;
  /** Draws the account's second-step page in the frame, once, when it is first reached; the second step's code is not read before that. */
  function drawSecond() {
    secondDrawn ??= import('../../account-steps.js').then(({ secondStepView }) => {
      if (view.signal.aborted) return;
      secondPanel.append(...secondStepView({
        session: view.session,
        // The session here is whole already, so there is nothing to sign in again for.
        signInAgain: () => {},
        embedded: true,
        changed: async () => {
          try {
            chosen.second = hasSecondStep(await view.api.get('/api/1/account/second-step'));
          } catch {
            return;
          }
          marks[2].done = chosen.second;
          drawMarks();
          drawButtons();
        },
      }));
    });
    return secondDrawn;
  }

  // ---- The buttons ----
  const back = h('button', { class: 'btn', type: 'button' }, 'Back');
  const next = h('button', { class: 'btn default', type: 'button' }, 'Next');
  const skip = h('button', { class: 'btn', type: 'button' }, 'Skip for now');
  const skipAll = h('button', { class: 'btn link', type: 'button' }, 'Skip all');
  const everyButton = [back, next, skip, skipAll];
  const last = () => index === steps.length - 1;
  const drawButtons = () => {
    back.hidden = index === 0;
    next.textContent = last() ? 'Finish' : 'Next';
    // A second step that is added has nothing to skip.
    skip.hidden = steps[index].id === 'second' && chosen.second;
  };

  const panels = { zone: zonePanel, display: displayPanel, ...(secondPanel ? { second: secondPanel } : {}) };
  const headings = { zone: zoneHeading, display: displayHeading, second: null };

  /** Shows step `to`; moving by a button puts focus on its heading, so that a screen reader says where it is. */
  function go(to, { focus = true } = {}) {
    index = to;
    for (const step of steps) panels[step.id].hidden = step.id !== steps[index].id;
    drawMarks();
    drawButtons();
    const id = steps[index].id;
    const focusHeading = () => (headings[id] ?? panels[id].querySelector('h2'))?.focus();
    if (id === 'second') {
      // Its heading is the account page's own, which is there once the page is read.
      drawSecond().then(() => { if (focus && steps[index].id === id) focusHeading(); });
    } else if (focus) {
      focusHeading();
    }
  }

  /** The end: the zone on the screen is saved if it is not yet, so that this page does not come back, and the person goes on. */
  async function finish() {
    if (!zoneSaved && !(await saveZone(shownZone()))) {
      go(0);
      return;
    }
    marks[0].done = true;
    view.previewDisplay(null);
    leave();
  }

  /** Where the first sign-in ends: the page that was asked for, or for an administrator with steps left, the offer to set the machine up. */
  async function leave() {
    const then = thenPath(view.query.get('then'));
    if (view.session.admin) {
      const left = (await readSteps(view).catch(() => [])).filter((step) => !step.done);
      if (view.signal.aborted) return;
      if (left.length > 0) {
        offer(left.length, then);
        return;
      }
    }
    view.go(then);
  }

  function offer(count, then) {
    const words = offerWords(host, count);
    const heading = h('h2', { tabindex: '-1' }, words.title);
    const set = h('button', { class: 'btn default', type: 'button' }, 'Set up');
    const later = h('button', { class: 'btn', type: 'button' }, 'Later');
    set.addEventListener('click', () => view.go('/setup'));
    later.addEventListener('click', () => view.go(then));
    heading.id = uniqueId('welcome-offer');
    wizard.replaceWith(h('section', { class: 'card offer', 'aria-labelledby': heading.id },
      heading,
      h('p', {}, words.detail),
      h('p', { class: 'muted' }, 'Later leaves them in Finish setting up on the Overview.'),
      h('div', { class: 'btns' }, set, later)));
    heading.focus();
  }

  next.addEventListener('click', () => busy(everyButton, async () => {
    const id = steps[index].id;
    if (id === 'zone') {
      if (!(await saveZone(shownZone()))) return;
      marks[0].done = true;
    } else if (id === 'display') {
      try {
        await view.setPrefs(chosen.display);
      } catch (error) {
        view.problem(error, {});
        return;
      }
      marks[1].done = true;
    }
    if (last()) await finish();
    else go(index + 1);
  }));
  skip.addEventListener('click', () => busy(everyButton, async () => {
    if (steps[index].id === 'display') {
      // What was previewed and not kept goes back, and so do the cards.
      view.previewDisplay(null);
      chosen.display = displayOf(view.prefs);
      drawDisplay();
    }
    if (last()) await finish();
    else go(index + 1);
  }));
  skipAll.addEventListener('click', () => busy(everyButton, finish));
  back.addEventListener('click', () => go(index - 1));

  // ---- The page ----
  const wizard = h('div', { class: 'wiz' },
    h('div', { class: 'steps-col card' }, h('ol', { class: 'steps', 'aria-label': 'Steps' }, ...marks.map((mark) => h('li', {}, mark.element)))),
    h('div', { class: 'form-col' },
      zonePanel, displayPanel, secondPanel,
      h('div', { class: 'btns' }, back, next, skip, h('span', { class: 'spacer' }), skipAll),
      h('p', { class: 'small muted' }, 'You can change these later under Your account. Skip all keeps the time zone shown and leaves the rest as it is.')));

  view.main.classList.add('welcome');
  view.main.append(
    h('p', { class: 'muted' }, `${stepsWords(steps.length)} before you start. Each can be skipped.`),
    wizard);

  drawZone();
  drawDisplay();
  // A browser whose zone the list does not have starts with the field open.
  if (!browser || !zone.valid()) showField();
  go(0, { focus: false });
}
