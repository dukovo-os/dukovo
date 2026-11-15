// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Show command (design 6), the front the shell holds from the start: the
// calls a page makes, and the Show commands switch. lib/command-work.js,
// which reads the calls and draws the panel and the drawer, is read the
// first time something needs it: a panel, the drawer, or a one-line button
// that is to be shown.

import { h } from '../dom.js';

/**
 * `get` and `post` are api.js's; `drawer` is the shell's. Gives `panel`,
 * `show`, `inline`, `setOn` and `reset`, which work as lib/command-work.js's.
 */
export function makeCommands(deps) {
  let work = null;
  let on = false;
  // The one-line buttons made while the switch is off, which need no code until it goes on.
  let waiting = [];

  const load = () => {
    work ??= import('./command-work.js').then(({ makeWork }) => {
      const made = makeWork(deps);
      made.setOn(on);
      return made;
    });
    return work;
  };

  return {
    panel(action, args = {}, options = {}) {
      const holder = h('div', { class: 'cmd-holder' }, h('p', { class: 'small muted' }, 'Loading the command…'));
      load().then(
        (made) => holder.replaceChildren(made.panel(action, args, options)),
        () => holder.replaceChildren(h('p', { class: 'msg' }, 'The panel could not be loaded. Try again.')),
      );
      return holder;
    },
    show: (action, args = {}, opener = document.activeElement) => load().then((made) => made.show(action, args, opener)),
    inline(action, args = {}, { always = false } = {}) {
      // Hidden until it has the command's words: an empty button has no name.
      const element = h('button', { class: 'inline-cmd', type: 'button', hidden: true });
      if (on || always) load().then((made) => made.adopt(element, action, args, always));
      else waiting.push({ element, action, args });
      return element;
    },
    setOn(next) {
      on = next;
      if (work) load().then((made) => made.setOn(next));
      if (on && waiting.length > 0) {
        const adopt = waiting;
        waiting = [];
        load().then((made) => adopt.forEach(({ element, action, args }) => made.adopt(element, action, args, false)));
      }
    },
    reset() {
      waiting = [];
      work?.then((made) => made.reset());
    },
  };
}
