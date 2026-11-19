// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The header's chip for a trial that is running (C5 design 3.4): what is
// being tried, the time left and a link to the page that owns it, on every
// page but that one, whose banner says the same. With more than one trial
// the chip is a menu of them. admind sends the list of trials in the `trial`
// event; the chip counts down from it by this browser's clock, which the next
// list corrects. It says nothing aloud, since the page that owns the trial has
// the polite region. The words are lib/trial-model.js's.

import { h, icon } from '../dom.js';
import { chipWords, heldTrials, stillShown } from '../lib/trial-model.js';
import { menu } from './menus.js';

/** How often the countdown is drawn. */
const TICK_MS = 1000;

/** The icon, the words and the clock that the chip's link and its menu's button each draw. */
function face() {
  const words = h('span', { class: 'words' });
  const clock = h('span', { class: 'clock', 'aria-hidden': 'true' });
  return { words, clock, content: [icon('warn'), words, clock] };
}

/**
 * Makes the chip in `slot`, a hidden element of the header, from `shell`'s
 * `time()`, `bus` and `signal`, which stops the countdown. Gives
 * `{show(list)}`: `list` is the `trial` event's trials, every one that runs.
 */
export function trialChip(slot, shell) {
  let held = [];
  let timer = null;
  const current = () => ({ route: location.pathname, now: Date.now(), clock: (seconds) => shell.time().clock(seconds) });

  const linkFace = face();
  const link = h('a', { class: 'topbtn trial-chip' }, ...linkFace.content);
  const menuFace = face();
  const several = menu({
    label: 'Changes being tried',
    content: menuFace.content,
    items: () => chipWords(held, current())?.items ?? [],
  });
  several.button.classList.add('trial-chip');
  slot.append(link, several.element);

  // A tick sets only what changed, so that the page is not touched for nothing.
  const set = (node, text) => {
    if (node.textContent !== text) node.textContent = text;
  };
  const put = (node, name, value) => {
    if (node.getAttribute(name) !== value) node.setAttribute(name, value);
  };

  function draw() {
    const at = current();
    held = stillShown(held, at.now);
    const words = chipWords(held, at);
    slot.hidden = words === null;
    if (held.length === 0) {
      clearInterval(timer);
      timer = null;
    }
    if (words === null) return;
    link.hidden = words.href === null;
    several.element.hidden = words.href !== null;
    if (words.href === null) {
      set(menuFace.words, `${words.words} ·`);
      set(menuFace.clock, words.clock);
      put(several.button, 'aria-label', words.label);
    } else {
      put(link, 'href', words.href);
      set(linkFace.words, `${words.words} ·`);
      set(linkFace.clock, words.clock);
      put(link, 'aria-label', words.label);
    }
  }

  shell.bus.on('route', draw);
  shell.signal.addEventListener('abort', () => clearInterval(timer));

  return {
    show(list) {
      held = heldTrials(list, Date.now());
      if (held.length > 0) timer ??= setInterval(draw, TICK_MS);
      draw();
    },
  };
}
