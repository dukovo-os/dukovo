// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/shell drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): the console's
// frame, which every page of it sits in. An administrator signs in and finds
// the header, the navigation and the page in their landmarks; the skip link
// and the keys reach each of them; each route puts focus on the page's h1 and
// its name in the tab's title; the header and its menus work by the arrow
// keys; a display choice applies at once, stays after a reload and follows
// the person to a second browser; the text at 150% does not scroll sideways at
// 1280 and at 390 wide; the unlock's states and its warning two minutes before
// the end show in the header and in a notice; a console that does not answer
// is said so in a banner and is back at one press; a session signed out in
// another tab shows the sign-in page; a reader is told which pages are for
// administrators; at phone width the tab bar and the More sheet do what the
// header and the navigation do wider, with Alerts as the fourth tab of an
// administrator's bar; and eight tabs of one browser context,
// each on a page that reads, all show their content with no error view, and one
// more read in the first tab is answered (the tabs share the browser's
// connections to the console, which HTTP/2 lets each tab's event stream and
// reads use together).
//
// Every icon of the navigation is drawn: its mask image is a file under
// /icons/ that the browser asked for and the console serves as an image.
//
// Each look is the console's own: dark, light, high contrast, forced colours
// and less motion, each checked by axe (console.mjs's lookAll). Each engine
// has an administrator of its own, and there is one reader, which shell.toml
// made, so no script needs the lab's password. The header's chip for a change
// being tried is shell-trial.mjs's, run by later steps of shell.toml.
//
// The lab's admind has no short unlock time, so the warning two minutes
// before the end is shown by answering one call for the session with fewer
// seconds than admind says, as the page would read them; every other answer
// is admind's.

import {
  accountItem,
  ensure,
  fetchAsPage,
  leaveWelcome,
  look,
  lookAll,
  mustHaveCalled,
  openAccountMenu,
  openConsole,
  policyStopped,
  press,
  screenshot,
  sessionOf,
  sidewaysScroll,
  signIn,
  signInDone,
  signInHeading,
  signOut,
  signOutButton,
  sleep,
} from "./console.mjs";

// The password that shell.toml gives the accounts it makes.
const PASSWORD = "lab-console-password";

// An account in group console that is no administrator.
const READER = "wc-shell-reader";

// The phone of section 14: 390 by 844.
const PHONE = { width: 390, height: 844 };

// The calls that lab/coverage.toml says this test covers, which the browser
// must have made by the end.
const ROUTES = [
  "GET /api/1/session",
  "GET /api/1/account/preferences",
  "POST /api/1/account/preferences",
  "GET /api/1/events",
  "POST /api/1/unlock",
  "POST /api/1/unlock/extend",
  "POST /api/1/lock",
  "GET /api/1/health",
];

// The calls the eight tabs make between them: each page's own read, which the
// browser must have made by the end of that step.
const TAB_ROUTES = [
  "GET /api/1/events",
  "GET /api/1/services",
  "GET /api/1/network",
  "GET /api/1/firewall",
  "GET /api/1/logs",
  "GET /api/1/terminals",
  "POST /api/1/terminals",
  "GET /api/1/account/preferences",
];

/** The tab's title, which names the page, the machine and the console. */
const titleOf = (page) => page.title();

/** The name of the element that has focus: its label or its words. */
const focusedName = (page) =>
  page.evaluate(() => {
    const element = document.activeElement;
    return (element.getAttribute("aria-label") || element.innerText || element.textContent || "").trim().replace(/\s+/g, " ");
  });

/** Waits for focus to be on the h1 of the page, which is where each route leaves it. */
const seeFocusOnHeading = (page, words) =>
  page.waitForFunction(
    (wanted) => {
      const heading = document.querySelector("main h1");
      return heading !== null && document.activeElement === heading && heading.textContent.trim() === wanted;
    },
    words,
    { timeout: 15_000 },
  );

/** The names of the links in the navigation that a person sees, in order. */
const navLinks = (page) => page.getByRole("navigation", { name: "Pages" }).getByRole("link").allInnerTexts();

/** The card of the display choice `label` in `display`, which a person presses to choose it. */
const choice = (display, label) => display.getByText(label, { exact: true });

/** The root's classes, which say how the console looks. */
const rootClasses = (page) => page.evaluate(() => [...document.documentElement.classList]);

/** The saved preferences, as the console answers them to the page. */
async function preferencesOf(page) {
  const answer = await page.request.get(new URL("/api/1/account/preferences", page.url()).href);
  ensure(answer.status() === 200, `GET /api/1/account/preferences answered ${answer.status()}`);
  return answer.json();
}

/** The sign-in page, and the five looks of it. */
async function signedOut(t, page) {
  await t.see(signInHeading(page));
  await lookAll(t, page, "sign-in");
}

/** The signed-in page has its landmarks, each once: the banner, the navigation, the main part. */
async function landmarks(t, page, host) {
  await t.see(page.getByRole("banner"));
  ensure((await page.getByRole("banner").count()) === 1, "the page has more than one banner");
  ensure((await page.getByRole("main").count()) === 1, "the page has more than one main part");
  ensure(
    (await page.getByRole("navigation", { name: "Pages" }).count()) === 1,
    "a person sees more or fewer than one navigation called Pages",
  );
  await t.see(page.getByRole("heading", { name: "Overview", level: 1 }));
  await seeFocusOnHeading(page, "Overview");
  ensure((await titleOf(page)) === `Overview · ${host} · Dukovo Admin`, `the tab is called "${await titleOf(page)}"`);
  await lookAll(t, page, "overview");
}

/** The navigation lists the pages in the order of the design, with the current one marked. */
async function navigation(t, page, session) {
  const names = (await navLinks(page)).map((text) => text.replace(/\s+/g, " ").trim());
  // The terminal's entry is there when the account has a terminal, and
  // Report a problem is last, apart under the rule.
  const pages = [
    "Overview", "Logs", "Reliability", "Storage", "Snapshots and backups", "Networking", "Firewall", "Accounts",
    "Services", "Apps", "Certificates", "Security", "Hardware", "Updates",
  ];
  const wanted = [...pages, ...(session.terminal ? ["Terminal"] : []), "Report a problem"];
  ensure(JSON.stringify(names) === JSON.stringify(wanted), `the navigation lists ${names.join(", ")}, not ${wanted.join(", ")}`);
  const current = page.getByRole("navigation", { name: "Pages" }).getByRole("link", { name: "Overview" });
  ensure((await current.getAttribute("aria-current")) === "page", "the navigation does not mark the Overview as the current page");
}

/**
 * Every icon of the navigation is drawn: each is a mask whose image is a
 * file under /icons/, which the browser has asked for and the console serves
 * as an image. An icon whose file did not arrive has an empty mask and is not
 * drawn at all, which no other check here would see.
 */
async function navigationIcons(t, page) {
  const icons = await page.getByRole("navigation", { name: "Pages" }).locator("i.ic").evaluateAll((nodes) =>
    nodes.map((node) => {
      const style = getComputedStyle(node);
      const mask = [style.maskImage, style.webkitMaskImage].find((value) => value && value !== "none") ?? "none";
      const box = node.getBoundingClientRect();
      return {
        name: [...node.classList].find((word) => word.startsWith("ic-")) ?? node.className,
        url: /url\(["']?([^"')]+)["']?\)/.exec(mask)?.[1] ?? "",
        size: Math.min(box.width, box.height),
      };
    }),
  );
  const links = (await navLinks(page)).length;
  ensure(icons.length >= links, `the navigation has ${links} links and draws ${icons.length} icons`);
  for (const icon of icons) {
    ensure(icon.url !== "", `the icon ${icon.name} has no mask image`);
    ensure(new URL(icon.url, page.url()).pathname === `/icons/${icon.name.slice(3)}.svg`, `the icon ${icon.name} has the mask image ${icon.url}`);
    ensure(icon.size > 0, `the icon ${icon.name} has no size`);
  }
  const urls = [...new Set(icons.map((icon) => new URL(icon.url, page.url()).href))];
  // The browser asked for each file for the mask, before this check asks for it again.
  const asked = await page.evaluate(() => performance.getEntriesByType("resource").map((entry) => entry.name));
  const missing = urls.filter((url) => !asked.includes(url));
  ensure(missing.length === 0, `the browser did not ask for the image of ${missing.join(", ")} (it kept the timing of ${asked.length} files)`);
  const served = await page.evaluate(
    (wanted) =>
      Promise.all(
        wanted.map(async (url) => {
          const answer = await fetch(url, { credentials: "same-origin" });
          const text = await answer.text();
          return { url, status: answer.status, type: answer.headers.get("content-type") ?? "", svg: text.includes("<svg") };
        }),
      ),
    urls,
  );
  for (const file of served) {
    ensure(file.status === 200, `${file.url} was answered ${file.status}`);
    ensure(file.type.startsWith("image/svg+xml"), `${file.url} is served as ${file.type}`);
    ensure(file.svg, `${file.url} is not a drawing`);
  }
}

/** The skip link is the first thing the keys reach, shows when it has focus, and puts focus in the page. */
async function skipLink(t, page) {
  // Focus is on the h1 after the route; Shift+Tab goes back to the start.
  let name = "";
  for (let tries = 0; tries < 30 && name !== "Skip to the page"; tries += 1) {
    await page.keyboard.press("Shift+Tab");
    name = await focusedName(page);
  }
  ensure(name === "Skip to the page", `Shift+Tab never reached the skip link: it reached "${name}"`);
  const link = page.getByRole("link", { name: "Skip to the page" });
  await t.see(link);
  const box = await link.boundingBox();
  ensure(box !== null && box.y >= 0 && box.height > 0, "the skip link has focus and is not on the screen");
  await page.keyboard.press("Enter");
  const inMain = await page.evaluate(() => document.getElementById("main").contains(document.activeElement));
  ensure(inMain, "the skip link did not put focus in the main part");
}

/** The keys go through the header in its order: the skip link, the brand, the host, Tasks, Help and the account. */
async function headerByKeys(t, page, host) {
  await page.reload();
  await seeFocusOnHeading(page, "Overview");
  let name = "";
  for (let tries = 0; tries < 30 && name !== "Skip to the page"; tries += 1) {
    await page.keyboard.press("Shift+Tab");
    name = await focusedName(page);
  }
  const seen = [name];
  for (let tries = 0; tries < 12; tries += 1) {
    await page.keyboard.press("Tab");
    seen.push(await focusedName(page));
  }
  const wanted = [
    "Skip to the page",
    "Dukovo Admin, Overview",
    `Server ${host}`,
    "Tasks",
    "Help",
    " account menu",
  ];
  let from = 0;
  for (const words of wanted) {
    const at = seen.findIndex((each, index) => index >= from && (words.startsWith(" ") ? each.includes(words.trim()) : each.startsWith(words)));
    ensure(at >= 0, `the keys went through ${seen.join(" | ")}, which has no "${words}" after position ${from}`);
    from = at + 1;
  }
}

/** A menu opens with the arrow keys, moves with them, Home and End, and closes with Esc with focus on its button. */
async function menuByKeys(t, page, button, menu, first, last) {
  await button.focus();
  await page.keyboard.press("ArrowDown");
  await t.see(menu);
  ensure((await button.getAttribute("aria-expanded")) === "true", "the menu button does not say it is open");
  const opened = await focusedName(page);
  ensure(opened.length > 0 && (await page.evaluate(() => document.activeElement.getAttribute("role"))).startsWith("menuitem"), `focus is on "${opened}", not on an item`);
  await page.keyboard.press("Home");
  ensure((await focusedName(page)).includes(first), `Home did not reach "${first}"`);
  await page.keyboard.press("End");
  ensure((await focusedName(page)).includes(last), `End did not reach "${last}"`);
  await page.keyboard.press("ArrowDown");
  ensure((await focusedName(page)).includes(first), `ArrowDown at the end did not go round to "${first}"`);
  await page.keyboard.press("ArrowUp");
  ensure((await focusedName(page)).includes(last), `ArrowUp at the start did not go round to "${last}"`);
  await page.keyboard.press("Escape");
  await t.gone(menu);
  ensure((await button.getAttribute("aria-expanded")) === "false", "the menu button says it is open after Esc");
  ensure((await page.evaluate(() => document.activeElement.getAttribute("aria-haspopup"))) === "menu", "focus is not back on the menu's button after Esc");
}

/** The account menu and the Help menu, by the keys. */
async function menus(t, page) {
  const account = signOutButton(page);
  await menuByKeys(t, page, account, page.getByRole("menu", { name: /^Account of / }), "Your account", "Sign out");
  const help = page.getByRole("banner").getByRole("button", { name: "Help", exact: true });
  await menuByKeys(t, page, help, page.getByRole("menu", { name: "Help" }), "Keyboard shortcuts", "About Dukovo Admin");
}

/** The keys' dialog opens from the Help menu and from Ctrl+/, and closes with focus where it came from. */
async function shortcutsDialog(t, page) {
  const help = page.getByRole("banner").getByRole("button", { name: "Help", exact: true });
  await help.focus();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  const dialog = page.getByRole("dialog", { name: "Keyboard shortcuts" });
  await t.see(dialog);
  await look(t, page, "shortcuts");
  await page.keyboard.press("Escape");
  await t.gone(dialog);
  ensure((await focusedName(page)) === "Help", `focus is on "${await focusedName(page)}" after the dialog, not on Help`);
  await page.keyboard.press("Control+/");
  await t.see(dialog);
  await dialog.getByRole("button", { name: "Close" }).click();
  await t.gone(dialog);
}

/** Each route puts focus on its h1 and its name in the tab's title; the browser's Back does too. */
async function routes(t, page, host) {
  const menu = await openAccountMenu(t, page);
  await accountItem(menu, "Your account").click();
  await t.see(page.getByRole("heading", { name: "Your account", level: 1 }));
  await seeFocusOnHeading(page, "Your account");
  ensure((await titleOf(page)) === `Your account · ${host} · Dukovo Admin`, `the tab is called "${await titleOf(page)}"`);
  ensure(new URL(page.url()).pathname === "/account", `the address is ${page.url()}`);
  await lookAll(t, page, "account");

  await page.getByRole("link", { name: "Dukovo Admin, Overview" }).click();
  await seeFocusOnHeading(page, "Overview");
  await page.goBack();
  await seeFocusOnHeading(page, "Your account");
  await page.goForward();
  await seeFocusOnHeading(page, "Overview");

  // A path that no page has is the console's own page, and says so.
  await page.goto(new URL("/no-such-page", page.url()).href);
  await seeFocusOnHeading(page, "No page here");
  await t.see(page.getByText("/no-such-page", { exact: true }));
  await look(t, page, "no-page");
  await page.getByRole("link", { name: "Go to the Overview" }).click();
  await seeFocusOnHeading(page, "Overview");
}

/** The display choices of the account menu and of the account page apply at once and are kept. */
async function displayChoices(t, page, second) {
  const choose = async (label) => {
    const menu = await openAccountMenu(t, page);
    await press(page, menu.getByRole("menuitemradio", { name: label, exact: true }), "/api/1/account/preferences");
  };
  await choose("Light");
  ensure((await rootClasses(page)).includes("light"), "the console is not light after the choice");
  ensure((await preferencesOf(page)).theme === "light", "the console did not keep the choice of light");
  await page.reload();
  await seeFocusOnHeading(page, "Overview");
  ensure((await rootClasses(page)).includes("light"), "the console is not light after a reload");

  // Another browser, signed in as the same account, is light from its first page.
  const other = await second();
  ensure((await rootClasses(other)).includes("light"), "a second browser is not light: the choice did not follow the person");
  await other.context().close();

  await choose("High contrast");
  ensure((await rootClasses(page)).includes("hc"), "the console is not in high contrast after the choice");
  await look(t, page, "high-contrast-chosen");
  await choose("Dark");
  ensure((await rootClasses(page)).includes("dark"), "the console is not dark after the choice");

  // The page of the account has the same choices, with the size and the motion.
  await accountItem(await openAccountMenu(t, page), "Your account").click();
  await seeFocusOnHeading(page, "Your account");
  const display = page.getByRole("region", { name: "Display" });
  await press(page, choice(display, "125%"), "/api/1/account/preferences");
  ensure((await rootClasses(page)).includes("t125"), "the text is not 125% after the choice");
  await press(page, choice(display, "Less"), "/api/1/account/preferences");
  ensure((await rootClasses(page)).includes("calm"), "the console does not have less motion after the choice");
  const kept = await preferencesOf(page);
  ensure(kept.text === 125 && kept.motion === "less", `the console kept ${JSON.stringify(kept)}`);
  await page.reload();
  await seeFocusOnHeading(page, "Your account");
  const classes = await rootClasses(page);
  ensure(classes.includes("t125") && classes.includes("calm"), `after a reload the console is ${classes.join(" ")}`);
  await press(page, choice(display, "100%"), "/api/1/account/preferences");
  await press(page, choice(display, "Full"), "/api/1/account/preferences");
  await page.getByRole("link", { name: "Dukovo Admin, Overview" }).click();
  await seeFocusOnHeading(page, "Overview");
}

/** At 150% text the page does not scroll sideways, and the page still has its parts. */
async function largeText(t, page) {
  await accountItem(await openAccountMenu(t, page), "Your account").click();
  await seeFocusOnHeading(page, "Your account");
  const display = page.getByRole("region", { name: "Display" });
  await press(page, choice(display, "150%"), "/api/1/account/preferences");
  ensure((await rootClasses(page)).includes("t150"), "the text is not 150% after the choice");
  ensure((await sidewaysScroll(page)) === 0, `at 150% the account page scrolls ${await sidewaysScroll(page)} px sideways`);
  await screenshot(t, page, "account-text-150");
  await page.getByRole("link", { name: "Dukovo Admin, Overview" }).click();
  await seeFocusOnHeading(page, "Overview");
  ensure((await sidewaysScroll(page)) === 0, `at 150% the Overview scrolls ${await sidewaysScroll(page)} px sideways`);
  await screenshot(t, page, "overview-text-150");
  await accountItem(await openAccountMenu(t, page), "Your account").click();
  await seeFocusOnHeading(page, "Your account");
  await press(page, choice(display, "100%"), "/api/1/account/preferences");
  await page.getByRole("link", { name: "Dukovo Admin, Overview" }).click();
  await seeFocusOnHeading(page, "Overview");
}

/** The same account's choice, 150%, at the phone's width: nothing scrolls sideways, and the tab bar is whole. */
async function largeTextOnPhone(t, page, name) {
  await t.see(page.getByRole("navigation", { name: "Pages" }));
  ensure((await page.getByRole("link", { name: "Overview" }).count()) > 0, `${name}: the phone shows no Overview in its tab bar`);
  ensure((await sidewaysScroll(page)) === 0, `at 150% on a phone the page scrolls ${await sidewaysScroll(page)} px sideways`);
  const bar = await page.getByRole("navigation", { name: "Pages" }).boundingBox();
  const size = page.viewportSize();
  ensure(bar !== null && bar.x >= 0 && bar.x + bar.width <= size.width + 1, "at 150% the tab bar is wider than the phone");
  await screenshot(t, page, "phone-text-150");
}

/** The header's states while the console is locked and unlocked, and its warning two minutes before the end. */
async function unlockStates(t, page) {
  const header = page.getByRole("banner");
  await t.see(header.getByText("Limited access"));
  await header.getByRole("button", { name: "Unlock", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Unlock to make changes" });
  await t.see(dialog);
  await dialog.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await press(page, dialog.getByRole("button", { name: "Unlock", exact: true }), "/api/1/unlock");
  await t.see(header.getByText("Unlocked ·"));
  await t.see(header.getByText("15 min", { exact: true }));
  await look(t, page, "unlocked");

  // The page reads the session again and is told it has two minutes, by
  // the session's answer and by the event stream, which tells the unlock
  // as soon as it opens: a stream left alone would say 15 minutes after
  // the answer said two, or before it.
  const answer = (route, seconds) => route.fetch().then(async (real) => {
    const body = await real.json();
    return route.fulfill({ response: real, json: { ...body, unlock_seconds: seconds } });
  });
  const stream = (route, seconds) => route.fulfill({
    status: 200,
    contentType: "text/event-stream",
    headers: { "Cache-Control": "no-store" },
    body: `event: unlock\ndata: ${JSON.stringify({ unlock_seconds: seconds })}\n\n`,
  });
  await page.route("**/api/1/session", (route) => answer(route, 110));
  await page.route("**/api/1/events", (route) => stream(route, 110));
  await page.reload();
  await t.see(header.getByText("Unlocked ·"));
  await t.see(header.getByText("2 min", { exact: true }));
  const warning = page.getByRole("status").filter({ hasText: "The console locks in 2 minutes" });
  await t.see(warning);
  await look(t, page, "locks-soon");
  await page.unroute("**/api/1/session");

  // The notice has the button that the header has, and it stays until it is used.
  await sleep(9000);
  await t.see(warning);
  // The stream the browser opens again from here on is the console's own.
  await page.unroute("**/api/1/events");
  await press(page, warning.getByRole("button", { name: "Reset to 15 min" }), "/api/1/unlock/extend");
  await t.gone(warning);
  await t.see(header.getByText("15 min", { exact: true }));
  await t.gone(header.getByText("2 min", { exact: true }));

  await press(page, header.getByRole("button", { name: "Lock", exact: true }), "/api/1/lock");
  await t.see(header.getByText("Limited access"));
}

/** A console that does not answer is said so in a banner, and the page is back when it does. */
async function offline(t, page) {
  await accountItem(await openAccountMenu(t, page), "Your account").click();
  await seeFocusOnHeading(page, "Your account");
  const display = page.getByRole("region", { name: "Display" });
  const context = page.context();
  await context.route("**/api/**", (route) => route.abort("connectionrefused"));
  await choice(display, "Light").click();
  const banner = page.getByRole("status").filter({ hasText: "is not answering" });
  await t.see(banner);
  await look(t, page, "offline");
  // The choice the console could not keep is undone.
  ensure(!(await rootClasses(page)).includes("light"), "a choice that the console could not keep is still shown");

  await context.unroute("**/api/**");
  await banner.getByRole("button", { name: "Try now" }).click();
  await t.gone(banner);
  await t.see(page.getByRole("heading", { name: "Your account", level: 1 }));
  await press(page, choice(display, "Light"), "/api/1/account/preferences");
  await press(page, choice(display, "Dark"), "/api/1/account/preferences");
}

/** A session signed out in another tab shows the sign-in page in this one, with the reason. */
async function signedOutElsewhere(t, page) {
  const other = await page.context().newPage();
  await other.goto(new URL("/", page.url()).href);
  await t.see(signOutButton(other));
  await signOut(t, other);
  await t.see(signInHeading(page));
  await t.see(page.getByText("you signed out", { exact: false }));
  await look(t, page, "signed-out-elsewhere");
  await other.close();
}

/** A reader is offered no unlock, and is told which pages are for administrators. */
async function reader(t, page) {
  const answer = await signIn(page, READER, PASSWORD);
  ensure(answer.status() === 200, `${READER} was not signed in: the console answered ${answer.status()}`);
  await t.see(signOutButton(page));
  await leaveWelcome(t, page);
  const header = page.getByRole("banner");
  await t.see(header.getByText("Limited access"));
  await t.gone(header.getByRole("button", { name: "Unlock", exact: true }));
  await page.goto(new URL("/setup", page.url()).href);
  await seeFocusOnHeading(page, "Set up this server");
  await t.see(page.getByText("This page is for administrators."));
  await lookAll(t, page, "administrators-only");
  await signOut(t, page);
}

/** At 390 wide: the tab bar holds the pages and More, the sheet holds the rest, and every target is 44 px. */
async function phone(t, name) {
  const { page, seen } = await openConsole(t, "/", { viewport: PHONE });
  // The account menu is in the More sheet here, so the page is signed in when the tab bar is there.
  const answer = await signIn(page, name, PASSWORD);
  ensure(answer.status() === 200, `${name} was not signed in on a phone: the console answered ${answer.status()}`);
  await t.see(page.getByRole("button", { name: "More", exact: true }));
  await leaveWelcome(t, page);
  await t.see(page.getByRole("navigation", { name: "Pages" }));
  await t.see(page.getByRole("heading", { name: "Overview", level: 1 }));
  await seeFocusOnHeading(page, "Overview");
  ensure((await sidewaysScroll(page)) === 0, `the phone's page scrolls ${await sidewaysScroll(page)} px sideways`);

  // The bar holds four pages, and an administrator's fourth is Alerts, which the navigation of a computer does not list; More follows them.
  const tabs = await page.locator("nav.tabbar a").evaluateAll((links) => links.map((link) => link.textContent.trim()));
  ensure(tabs.join("|") === "Overview|Services|Logs|Alerts", `the phone's tab bar holds ${tabs.join(", ")}, not Overview, Services, Logs and Alerts`);
  await t.see(page.locator("nav.tabbar").getByRole("button", { name: "More", exact: true }));

  // Every control a thumb presses is at least 44 px each way.
  const small = await page.evaluate(() =>
    [...document.querySelectorAll("button, a[href], input, select, summary")]
      .filter((element) => element.offsetParent !== null && !element.classList.contains("skip"))
      .map((element) => ({ name: element.getAttribute("aria-label") || element.textContent.trim() || element.className, box: element.getBoundingClientRect() }))
      .filter(({ box }) => box.width > 0 && (box.height < 43.5 || box.width < 43.5))
      .map(({ name, box }) => `${name} (${Math.round(box.width)} by ${Math.round(box.height)})`),
  );
  ensure(small.length === 0, `these controls are under 44 px on a phone: ${small.join(", ")}`);
  await lookAll(t, page, "phone");

  // More holds the pages and what the account and Help menus hold wider.
  await page.getByRole("button", { name: "More", exact: true }).click();
  const sheet = page.getByRole("dialog", { name: "More" });
  await t.see(sheet);
  await t.see(sheet.getByRole("link", { name: "Your account", exact: true }));
  await t.see(sheet.getByRole("button", { name: "Sign out", exact: true }));
  await lookAll(t, page, "phone-more");
  await page.keyboard.press("Escape");
  await t.gone(sheet);

  // The unlock is one icon here, and asks in the same dialog.
  await page.getByRole("button", { name: "Limited access, unlock" }).click();
  const dialog = page.getByRole("dialog", { name: "Unlock to make changes" });
  await t.see(dialog);
  await look(t, page, "phone-unlock");
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await t.gone(dialog);

  // The text at 150% on the phone.
  await page.getByRole("button", { name: "More", exact: true }).click();
  await t.see(sheet);
  await press(page, sheet.getByRole("button", { name: "Light", exact: true }), "/api/1/account/preferences");
  ensure((await rootClasses(page)).includes("light"), "the phone's console is not light after the choice in More");
  await press(page, (await openMore(t, page)).getByRole("button", { name: "Dark", exact: true }), "/api/1/account/preferences");
  await page.goto(new URL("/account", page.url()).href);
  await seeFocusOnHeading(page, "Your account");
  const display = page.getByRole("region", { name: "Display" });
  await press(page, choice(display, "150%"), "/api/1/account/preferences");
  await largeTextOnPhone(t, page, name);
  await press(page, choice(display, "100%"), "/api/1/account/preferences");

  await signOut(t, page);
  mustHaveCalled(seen, ["GET /api/1/session", "POST /api/1/sign-out"]);
  await page.context().close();
}

/** Opens the More sheet and gives it. */
async function openMore(t, page) {
  const sheet = page.getByRole("dialog", { name: "More" });
  if (!(await sheet.isVisible())) await page.getByRole("button", { name: "More", exact: true }).click();
  await t.see(sheet);
  return sheet;
}

/**
 * The eight pages of the tabs, each one that reads and the words of the
 * heading it shows. `ready` runs in the page and says that its content is
 * there: what the page's own lab test waits for before it looks.
 */
const TABS = [
  {
    path: "/",
    heading: "Overview",
    ready: () => {
      const bodies = [...document.querySelectorAll(".ov .card-body")].filter((body) => body.offsetParent !== null);
      return bodies.length > 0 && bodies.every((body) => body.textContent.trim() !== "Loading…");
    },
  },
  {
    path: "/services",
    heading: "Services",
    ready: () => {
      const table = document.querySelector("table.svc-tbl");
      return table !== null && !table.hasAttribute("aria-busy") && table.querySelector("tbody tr[data-name]") !== null;
    },
  },
  {
    // diskhealth is the service that every image runs and nothing needs.
    path: "/services/diskhealth",
    heading: "diskhealth",
    ready: () => {
      const root = document.querySelector(".svc");
      return root !== null && !root.hasAttribute("aria-busy");
    },
  },
  {
    path: "/logs",
    heading: "Logs",
    ready: () => {
      const list = document.querySelector(".loglist");
      return list !== null && !list.hasAttribute("aria-busy") && document.querySelector(".lskel") === null;
    },
  },
  {
    path: "/networking",
    heading: "Networking",
    ready: () => document.querySelector(".net-card") !== null,
  },
  {
    path: "/firewall",
    heading: "Firewall",
    ready: () => {
      const table = document.querySelector(".fw-rules table");
      return table !== null && !table.hasAttribute("aria-busy");
    },
  },
  {
    path: "/terminal",
    heading: "Terminal",
    ready: () =>
      document.querySelector(".tr-pane") !== null &&
      [...document.querySelectorAll('[role="status"]')].some((status) => status.textContent.includes("Connected")),
  },
  {
    path: "/account",
    heading: "Your account",
    ready: () => document.querySelector("main section.card") !== null && document.querySelector("main [aria-busy='true']") === null,
  },
];

/**
 * Waits for `tab` to show the heading of `page` and its content, or an error
 * view, which fails the step with what it says.
 */
async function tabShows(t, tab, page) {
  await t.see(tab.getByRole("heading", { name: page.heading, level: 1, exact: true }));
  // The content, or an error view, whichever comes first. Each is waited for
  // as a function of its own: the content security policy forbids eval, so a
  // predicate made from a string cannot run in the page.
  const shown = tab.waitForFunction(page.ready, null, { timeout: 60_000 });
  const failed = tab.locator("main section.err").first().waitFor({ timeout: 60_000 });
  // The one that loses ends with its timeout, or when the tab closes.
  shown.catch(() => {});
  failed.catch(() => {});
  await Promise.race([shown, failed]);
  const errors = await tab.locator("main section.err h2").allInnerTexts();
  ensure(errors.length === 0, `the tab of ${page.heading} shows an error view: ${errors.join("; ")}`);
  const stopped = await policyStopped(tab);
  ensure(stopped.length === 0, `the content security policy stopped ${stopped.join(", ")} on the tab of ${page.heading}`);
}

/**
 * A GET of `route` from `tab`, as the page makes it, that gives up after
 * `milliseconds`: a read that waits for a connection another tab holds does
 * not come, and the step says so. Gives the status.
 */
function readWithin(tab, route, milliseconds) {
  return tab.evaluate(
    async ({ route, milliseconds }) => {
      try {
        const answer = await fetch(route, { credentials: "same-origin", signal: AbortSignal.timeout(milliseconds) });
        await answer.text();
        return answer.status;
      } catch (error) {
        return `${error.name}: ${error.message}`;
      }
    },
    { route, milliseconds },
  );
}

/**
 * Eight tabs of one browser context, each on a page that reads, load at
 * once. Each shows its content with no error view; then one more read in the
 * first tab is answered, which it is not when the tabs' event streams and
 * reads wait for each other's connections to the console.
 */
async function eightTabs(t, name) {
  const { page: first, seen } = await openConsole(t, "/");
  await signInDone(t, first, name, PASSWORD);
  const session = await sessionOf(first);
  ensure(session.terminal === true, `${name} has no Terminal, which the eighth tab needs: GET /api/1/session says ${session.terminal}`);

  const context = first.context();
  const tabs = [first];
  while (tabs.length < TABS.length) tabs.push(await context.newPage());
  ensure(context.pages().length === TABS.length, `the context has ${context.pages().length} tabs, not ${TABS.length}`);
  await Promise.all(tabs.map((tab, at) => tab.goto(new URL(TABS[at].path, first.url()).href)));
  await Promise.all(tabs.map((tab, at) => tabShows(t, tab, TABS[at])));

  // Every tab holds its event stream, and the first tab still reads.
  const status = await readWithin(first, "/api/1/services", 30_000);
  ensure(status === 200, `a read in the first tab, with eight tabs open, was answered ${status}`);
  mustHaveCalled(seen, TAB_ROUTES);

  // The tabs are closed, and the terminal the Terminal tab made is ended
  // (a page that is still open would make another).
  for (const tab of tabs.slice(1)) await tab.close();
  const terminals = await fetchAsPage(first, "GET", "/api/1/terminals");
  ensure(terminals.status === 200, `GET /api/1/terminals answered ${terminals.status}`);
  for (const terminal of terminals.json.terminals) {
    const ended = await fetchAsPage(first, "POST", `/api/1/terminals/${terminal.id}/end`, {});
    ensure(ended.status === 200, `the end of terminal ${terminal.id} was answered ${ended.status}: ${ended.text}`);
  }
  await signOut(t, first);
  await context.close();
}

export default async function check(t) {
  const name = `wc-shell-${t.engine}`;
  const { page, seen } = await openConsole(t, "/");
  await signedOut(t, page);
  await signInDone(t, page, name, PASSWORD);
  const session = await sessionOf(page);
  ensure(session.name === name && session.admin === true, `GET /api/1/session names ${session.name}, admin ${session.admin}`);

  await landmarks(t, page, session.host);
  await navigation(t, page, session);
  await navigationIcons(t, page);
  await skipLink(t, page);
  await headerByKeys(t, page, session.host);
  await menus(t, page);
  await shortcutsDialog(t, page);
  await routes(t, page, session.host);

  // A second browser for the same account, opened when the display choice
  // has been made: it signs in and shows what the account has chosen.
  const second = async () => {
    const other = await openConsole(t, "/");
    await signInDone(t, other.page, name, PASSWORD);
    return other.page;
  };
  await displayChoices(t, page, second);
  await largeText(t, page);
  await unlockStates(t, page);
  await offline(t, page);
  await signedOutElsewhere(t, page);

  await reader(t, page);
  await phone(t, name);
  await eightTabs(t, name);
  mustHaveCalled(seen, ROUTES);
}
