// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Puts the console's theme on the root element, as the mock-ups' own theme
// logic does for "Device setting": high contrast when the system asks for
// more contrast, else dark or light by its colour scheme. It is a plain
// script that the page loads before it draws, because a module runs after
// the first paint and a person who uses dark would see a flash of light.
(() => {
  const root = document.documentElement;
  const asksForContrast = matchMedia('(prefers-contrast: more)');
  const prefersDark = matchMedia('(prefers-color-scheme: dark)');
  const choose = () => {
    root.classList.remove('light', 'dark', 'hc');
    root.classList.add(asksForContrast.matches ? 'hc' : prefersDark.matches ? 'dark' : 'light');
  };
  choose();
  asksForContrast.addEventListener('change', choose);
  prefersDark.addEventListener('change', choose);
})();
