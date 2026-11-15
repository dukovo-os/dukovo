// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The front of the drawer (design 4.1) that the shell holds from the start;
// it reads shell/drawer.js, which does the work, when a drawer first opens.

/** The calls of `createDrawer`'s answer, with the same arguments. */
export function lazyDrawer(args) {
  const listeners = new Set();
  let made = null;
  const real = () => {
    made ??= import('./drawer.js').then(({ createDrawer }) => {
      const drawer = createDrawer(args);
      for (const listener of listeners) drawer.onChange(listener);
      return drawer;
    });
    return made;
  };
  // What has focus now gets it back, not what has it once the code is read.
  const fromHere = (options) => ({ opener: document.activeElement, ...options });
  return {
    open: (which, options) => real().then((drawer) => drawer.open(which, fromHere(options))),
    toggle: (which, options) => real().then((drawer) => drawer.toggle(which, fromHere(options))),
    onChange(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}
