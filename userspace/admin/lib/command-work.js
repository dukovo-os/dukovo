// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Show command (design 6), the working half: the drawer and the one-line
// buttons that show what a button does as a command, an API call, an Ansible
// task and the files it writes. `POST /api/1/show` is called once per action
// and arguments, and kept until `reset`. lib/command.js, which the shell
// holds from the start, reads this file when a page asks for one of them;
// the panel itself is lib/command-panel.js.

import { h } from '../dom.js';
import { showKey } from './command-model.js';

/**
 * `get` and `post` are api.js's; `drawer` is the shell's. Gives `panel`,
 * `show`, `inline`, `setOn` and `reset`.
 */
export function makeWork({ get, post, drawer }) {
  let shown = new Map();
  let cache = { journal: null, changes: new Map() };
  const inlines = new Set();
  let on = false;

  const load = (action, args) => {
    const key = showKey(action, args);
    if (!shown.has(key)) {
      const call = post('/api/1/show', { action, args });
      call.catch(() => shown.delete(key));
      shown.set(key, call);
    }
    return shown.get(key);
  };

  /** The panel for `action` with `args`: the tabs of the ShowCommand board, drawn once their code is read. */
  function panel(action, args = {}, options = {}) {
    const holder = h('div', { class: 'cmd-holder' }, h('p', { class: 'small muted' }, 'Loading the command…'));
    import('./command-panel.js').then(
      (module) => holder.replaceChildren(module.panel({ get, load, cache }, action, args, options)),
      () => holder.replaceChildren(h('p', { class: 'msg' }, 'The panel could not be loaded. Try again.')),
    );
    return holder;
  }

  /** Opens the drawer on the call for `action` and `args`. */
  function showDrawer(action, args = {}, opener = document.activeElement) {
    drawer.open('command', {
      title: 'Show command',
      opener,
      build(body) {
        const controller = new AbortController();
        body.replaceChildren(
          h('p', { class: 'small muted' }, 'What this button does, as a command, a call to the API and an Ansible task.'),
          panel(action, args, { signal: controller.signal }));
        return () => controller.abort();
      },
    });
  }

  /**
   * Wires `element`, the one-line button lib/command.js made under another
   * button, to the call for `action` and `args`: its text is the command,
   * and it opens the drawer on that call. It shows while the Show commands
   * switch is on, or always with `always`.
   */
  function adopt(element, action, args, always) {
    let loaded = false;
    const entry = { element, always, filled: false, fill: null };
    entry.fill = () => {
      if (loaded) return;
      loaded = true;
      load(action, args).then(
        (answer) => {
          element.textContent = answer.command;
          element.setAttribute('aria-label', `Show the command: ${answer.command}`);
          // Shown once it has its words, never empty: a button needs a name.
          entry.filled = true;
          element.hidden = !(on || always);
        },
        () => element.remove(),
      );
    };
    element.addEventListener('click', () => showDrawer(action, args, element));
    // The ones in the drawer outlive a route; those that it has closed are dropped here.
    for (const old of inlines) if (old.always && !old.element.isConnected) inlines.delete(old);
    inlines.add(entry);
    if (on || always) entry.fill();
  }

  return {
    panel,
    show: showDrawer,
    adopt,
    /** Shows or hides the lines of every button that has one: the Show commands switch. */
    setOn(next) {
      on = next;
      for (const entry of inlines) {
        entry.element.hidden = !(entry.filled && (on || entry.always));
        if (on) entry.fill();
      }
    },
    /** Forgets the calls made, so that the next page reads them again. */
    reset() {
      shown = new Map();
      cache = { journal: null, changes: new Map() };
      for (const entry of inlines) if (!entry.always) inlines.delete(entry);
    },
  };
}
