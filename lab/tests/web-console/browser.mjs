// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/browser drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): the page
// nginx serves, in dark and in light, and a page with a fault that the
// accessibility check must find.

export default async function check(t) {
  const backgrounds = {};
  for (const colorScheme of ["dark", "light"]) {
    const page = await t.open("/lab-browser.html", { colorScheme });
    await t.see(page.getByRole("heading", { name: "The lab can see this page" }));
    await page.getByRole("button", { name: "Press me" }).click();
    await t.see(page.getByText("Pressed", { exact: true }));
    backgrounds[colorScheme] = await page.evaluate(
      () => getComputedStyle(document.body).backgroundColor,
    );
    await t.axe(page);
    await t.screenshot(page, colorScheme);
  }
  // Both pairs of colours are in the page's own style sheet, so the same
  // background in both means the engine ignored the colour scheme asked.
  if (backgrounds.dark === backgrounds.light) {
    throw new Error(
      `the page looks the same in dark and light (${backgrounds.dark}): the browser did not apply the colour scheme`,
    );
  }

  const flawed = await t.open("/lab-browser-flawed.html");
  const ids = (await t.violations(flawed)).map(({ id }) => id);
  if (!ids.includes("image-alt")) {
    throw new Error(
      `the accessibility check missed a picture with no text; it found: ${ids.join(", ") || "nothing"}`,
    );
  }
}
