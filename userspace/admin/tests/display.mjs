// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A stand-in browser for theme.js, which is a classic script and so cannot
// be imported: it runs in a fresh context with a root element, media
// queries and localStorage that a test controls. Not a test itself.

import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const SOURCE = readFileSync(new URL('../theme.js', import.meta.url), 'utf8');

/**
 * Runs theme.js in a browser that asks for what `system` says
 * (`contrast`, `forced`, `dark`) and whose localStorage holds `kept` (a
 * string) or throws when `storage` is `'throws'`. Gives the browser's
 * parts and the `dukovoDisplay` the script made.
 */
export function browser({ system = {}, kept = null, storage = 'works' } = {}) {
  const classes = new Set(['dk']);
  const root = {
    classList: {
      add: (...names) => names.forEach((name) => classes.add(name)),
      remove: (...names) => names.forEach((name) => classes.delete(name)),
      contains: (name) => classes.has(name),
    },
  };
  const queries = {
    '(prefers-contrast: more)': { matches: Boolean(system.contrast), heard: [] },
    '(forced-colors: active)': { matches: Boolean(system.forced), heard: [] },
    '(prefers-color-scheme: dark)': { matches: Boolean(system.dark), heard: [] },
  };
  const matchMedia = (query) => {
    const asked = queries[query];
    return { get matches() { return asked.matches; }, addEventListener: (_, listener) => asked.heard.push(listener) };
  };
  const stored = new Map(kept === null ? [] : [['dukovo-admin.display.v1', kept]]);
  const localStorage = {
    getItem(key) {
      if (storage === 'throws') throw new Error('storage is blocked');
      return stored.get(key) ?? null;
    },
    setItem(key, value) {
      if (storage === 'throws') throw new Error('storage is blocked');
      stored.set(key, value);
    },
  };
  const context = { document: { documentElement: root }, matchMedia, localStorage };
  context.globalThis = context;
  vm.runInNewContext(SOURCE, context);
  // The script's arrays and objects come from its own context, which
  // deepEqual does not take for the test's own, so each answer is copied.
  const inside = context.dukovoDisplay;
  const display = {
    DEFAULTS: structuredClone(inside.DEFAULTS),
    clean: (shown) => structuredClone(inside.clean(shown)),
    classesFor: (shown, asks) => structuredClone(inside.classesFor(shown, asks)),
    apply: (shown) => structuredClone(inside.apply(shown)),
    read: () => structuredClone(inside.read()),
    keep: (shown) => inside.keep(shown),
  };
  return {
    display,
    classes: () => [...classes].sort(),
    stored,
    /** Changes what the device asks for, and tells those who listen, as the browser does. */
    systemChanges(query, matches) {
      queries[query].matches = matches;
      queries[query].heard.forEach((listener) => listener());
    },
  };
}
