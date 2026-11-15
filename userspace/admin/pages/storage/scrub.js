// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The buttons that scrub a volume (design 4.3 and 4.4). A scrub is a task that
// takes hours, so the button that began it shows how far it is, "Scrubbing…
// 41%", and opens the task when it is pressed. The page keeps one of these
// for all its buttons: the banner's and each disk's. Which scrubs run comes
// from the list of tasks when the page opens and from the task events after;
// the words on a button change in place, so that a button in hand stays in
// hand.

import { fill, h } from '../../dom.js';
import { makeScrubs, SCRUB_ACTION, scrubbingWords } from './model.js';

/** What the shell says of a scrub that was asked for. */
const WORDS = Object.freeze({ running: 'Scrubbing a volume', done: 'The scrub is done.', busy: 'Starting…' });

/**
 * `changed()` is called when a scrub ended, so that the page reads the volumes
 * again: the result and the disks that were behind are new. `repaint()` is
 * called when the state of a scrub changed, with no argument, for the places
 * that say it in other words than a button. Gives `{mounts, button, paint,
 * load}`.
 */
export function makeScrubber(view, { changed, repaint }) {
  const scrubs = makeScrubs();

  /** Puts the words that tell the state of each scrub button of the page. */
  function paint() {
    for (const button of view.main.querySelectorAll('[data-scrub]')) {
      if (button.getAttribute('aria-busy') === 'true') continue;
      const entry = scrubs.of(button.dataset.scrub);
      const words = entry ? scrubbingWords(entry) : button.dataset.label;
      if (button.dataset.shown === words) continue;
      button.dataset.shown = words;
      fill(button, words, entry && h('span', { class: 'sr-only' }, ' Opens the task.'));
    }
    repaint();
  }

  /** Reads the list of tasks, which says which scrubs run now. */
  async function load() {
    try {
      const answer = await view.api.get('/api/1/tasks');
      scrubs.learn(answer?.tasks);
      paint();
    } catch (error) {
      // The tasks are the Tasks list's to show; without them a button says what it always says.
      if (error?.name === 'AbortError') return;
    }
  }

  /** Reads one scrub whole: an event holds no arguments, and the mount is one. */
  async function learn(id) {
    try {
      scrubs.put(await view.api.get(`/api/1/tasks/${id}`));
      paint();
    } catch {
      // The next list has it.
    }
  }

  view.on('task', (data) => {
    const found = scrubs.event(data);
    if (found.learn !== undefined) learn(found.learn);
    paint();
    if (found.ended !== undefined) changed();
  });
  view.on('online', load);

  /** Runs the scrub of `mount`, or opens the one that runs. */
  async function press(button) {
    const { scrub: mount, label } = button.dataset;
    const running = scrubs.of(mount);
    if (running) {
      view.openTask(running.id);
      return;
    }
    view.slot.replaceChildren();
    const result = await view.api.run(SCRUB_ACTION, { mount }, { words: { ...WORDS, unlock: label.toLowerCase() }, button });
    if (typeof result.task === 'number') scrubs.started(result.task, mount);
    paint();
    if (result.ok && typeof result.task !== 'number') changed();
  }

  return {
    mounts: () => scrubs.mounts(),
    load,
    paint,
    /** A button that scrubs `mount`, with `label` when none runs, and the command it runs under it when Show commands is on. */
    button(mount, label, { kind = '' } = {}) {
      const button = h('button', { class: `btn${kind ? ` ${kind}` : ''}`, type: 'button', 'data-scrub': mount, 'data-label': label, 'data-shown': label }, label);
      button.addEventListener('click', () => press(button));
      const entry = scrubs.of(mount);
      if (entry) {
        button.dataset.shown = scrubbingWords(entry);
        button.replaceChildren(scrubbingWords(entry), h('span', { class: 'sr-only' }, ' Opens the task.'));
      }
      return h('div', { class: 'sto-action' }, button, view.api.inline(SCRUB_ACTION, { mount }));
    },
  };
}
