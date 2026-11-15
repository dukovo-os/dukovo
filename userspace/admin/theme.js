// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Puts the person's display choices on the root element as classes before
// the first paint: the colours (`dark`, `light` or `hc`), the text size
// (`t112`, `t125`, `t150`) and `calm` for less motion. It is a plain script,
// since a module runs after the first paint and a person who uses dark would
// see a flash of light.
//
// The choices come from the copy this browser kept in localStorage, since
// the server's are known only after sign-in; the shell then applies the
// server's and keeps them here. The copy can throw or be damaged, and then
// reads as the defaults, so the page always draws. The shell's preferences
// use `dukovoDisplay`, and the Node tests load it into a stand-in browser.
(() => {
  const KEY = 'dukovo-admin.display.v1';
  const THEMES = ['dark', 'light', 'high-contrast', 'device'];
  const TEXT_SIZES = [100, 112, 125, 150];
  const MOTIONS = ['full', 'less'];
  const DEFAULTS = { theme: 'dark', text: 100, motion: 'full' };
  const CLASSES = ['dark', 'light', 'hc', 't112', 't125', 't150', 'calm'];

  const root = document.documentElement;
  const asksForContrast = matchMedia('(prefers-contrast: more)');
  const forcedColours = matchMedia('(forced-colors: active)');
  const prefersDark = matchMedia('(prefers-color-scheme: dark)');

  /** `display` with each value the console has, and the default for one it does not. */
  function clean(display) {
    const given = display ?? {};
    return {
      theme: THEMES.includes(given.theme) ? given.theme : DEFAULTS.theme,
      text: TEXT_SIZES.includes(given.text) ? given.text : DEFAULTS.text,
      motion: MOTIONS.includes(given.motion) ? given.motion : DEFAULTS.motion,
    };
  }

  /**
   * The root's classes for `display`. `system` is what the device asks for:
   * `contrast`, `forced` and `dark`. Only "Same as this device" follows it;
   * a forced-colour theme overrides the others in the stylesheet.
   */
  function classesFor(display, system) {
    const { theme, text, motion } = clean(display);
    let colours = { dark: 'dark', light: 'light', 'high-contrast': 'hc' }[theme];
    if (theme === 'device') {
      colours = system.contrast || system.forced ? 'hc' : system.dark ? 'dark' : 'light';
    }
    const classes = [colours];
    if (text !== 100) classes.push(`t${text}`);
    if (motion === 'less') classes.push('calm');
    return classes;
  }

  let current = clean(null);

  function systemAsks() {
    return { contrast: asksForContrast.matches, forced: forcedColours.matches, dark: prefersDark.matches };
  }

  /** Makes the root's classes match `display`, and returns the choices it took. */
  function apply(display) {
    current = clean(display);
    root.classList.remove(...CLASSES);
    root.classList.add(...classesFor(current, systemAsks()));
    return current;
  }

  /** The choices this browser kept, or the defaults. */
  function read() {
    try {
      return clean(JSON.parse(localStorage.getItem(KEY)));
    } catch {
      return clean(null);
    }
  }

  /** Keeps `display` for this browser's next page, if the browser lets it. */
  function keep(display) {
    try {
      localStorage.setItem(KEY, JSON.stringify(clean(display)));
    } catch {
      // The next page starts from the defaults and then takes the server's.
    }
  }

  apply(read());
  const follow = () => current.theme === 'device' && apply(current);
  asksForContrast.addEventListener('change', follow);
  forcedColours.addEventListener('change', follow);
  prefersDark.addEventListener('change', follow);

  globalThis.dukovoDisplay = {
    KEY, THEMES, TEXT_SIZES, MOTIONS, DEFAULTS, clean, classesFor, apply, read, keep,
  };
})();
