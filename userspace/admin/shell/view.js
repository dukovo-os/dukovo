// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The view a page gets (design 3.3). Everything a page starts ends with its
// `signal`: fetches, timers and listeners. A page never touches the
// header, the navigation, the drawers or `document.title`: `head()` does.

import { drawHead } from './head.js';

/**
 * Makes the view for one route. `shell` is the shell's services (see
 * frame.js); `route` is `{path, rest, query, signal, ph, slot, page,
 * state}`, where `state.first` is true until the page's first content is
 * in place.
 */
export function makeView(shell, route) {
  const { signal } = route;
  const whileShown = (off) => signal.addEventListener('abort', off, { once: true });

  const view = {
    main: route.page,
    path: route.path,
    rest: route.rest,
    query: route.query,
    host: shell.host,
    signal,
    slot: route.slot,
    leaving: null,
    get session() {
      return shell.session();
    },
    get prefs() {
      return shell.prefs.value;
    },
    get time() {
      return shell.time();
    },

    /** Calls `listener` with the preferences whenever they change. */
    onPrefs(listener) {
      const off = shell.prefs.onChange(listener);
      whileShown(off);
      return off;
    },

    /** Draws the page's head and gives its h1. */
    head(options) {
      const head = drawHead({ region: route.ph, options, prefs: shell.prefs, host: shell.host });
      view.onPrefs(head.update);
      return head.h1;
    },

    api: {
      get: (path) => shell.get(path, { signal }),
      post: (path, body) => shell.post(path, body, { signal }),
      run: (action, args, options) => shell.run(signal)(action, args, options),
      show: (action, args, opener) => shell.commands.show(action, args, opener),
      panel: (action, args) => shell.commands.panel(action, args, { signal }),
      inline: (action, args) => shell.commands.inline(action, args),
    },

    /** Calls `listener` for each event of the stream named `event`, until the page is left. */
    on(event, listener) {
      const off = shell.bus.on(event, listener);
      whileShown(off);
      return off;
    },

    notice: (words, options) => shell.notice(words, options),

    /** Shows `error` in the place it belongs: see lib/problem.js. */
    problem(error, where = {}) {
      if (error?.name === 'AbortError') return;
      shell.problem(error, { first: route.state.first, ...where, slot: where.slot ?? route.slot });
    },

    /** Calls `fn` every `ms` while the page is shown, the tab is seen and the server answers. */
    every(ms, fn) {
      const timer = setInterval(() => {
        if (document.visibilityState === 'visible' && !shell.online.isOffline()) fn();
      }, ms);
      whileShown(() => clearInterval(timer));
      return () => clearInterval(timer);
    },

    go: (path) => shell.go(path),

    /** Opens the Tasks drawer with the log of the task `id` shown. */
    openTask: (id) => shell.tasks.open(document.activeElement, id),

    /** The parts named `name` of every page that has one, called with `call(module, cx)`. */
    parts: (name, call) => shell.parts.collect(name, call),

    /** Changes the person's preferences; the colours apply at once and go back if the server refuses. */
    setPrefs: (change) => shell.prefs.set(change),

    /** Shows other colours, size or motion without saving them; `null` shows the saved ones again. */
    previewDisplay: (display) => shell.prefs.preview(display),
  };
  return view;
}
