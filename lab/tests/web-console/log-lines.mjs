// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/logs and web-console/chart-to-logs share (lab repository,
// docs/test-files.md, "Browser tests"): the account's password and zone, the
// lines of the log as the API gives them and the rows of the page held to
// them, the times of a zone told by the browser's own Intl and not by the
// console's code, and the Show commands switch. The scripts that import this
// check the Logs page and the view from a chart, which both draw the same
// list of lines.

import { ensure, fetchAsPage, leaveWelcome, signIn, signOutButton } from "./console.mjs";

// The password that logs.toml gives the accounts it makes.
export const PASSWORD = "lab-console-password";

// The zone of the browser and of the account.
export const ZONE = "America/New_York";

// The phone of section 14: 390 by 844.
export const PHONE = { width: 390, height: 844 };

/** What one page of lines holds. */
export const PAGE_LINES = 200;

// A zone's clock is told the other way round from the console's, with the
// browser's own Intl, so that a time the page got wrong is not agreed with.
const WALL = new Map();
export function wallOf(ms, zone) {
  if (!WALL.has(zone)) {
    WALL.set(
      zone,
      new Intl.DateTimeFormat("en-GB", { timeZone: zone, hourCycle: "h23", year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", second: "2-digit", timeZoneName: "longOffset" }),
    );
  }
  const parts = Object.fromEntries(WALL.get(zone).formatToParts(new Date(ms)).map((part) => [part.type, part.value]));
  const found = /^GMT(?:([+-])(\d{2}):(\d{2}))?$/.exec(parts.timeZoneName);
  ensure(found !== null, `${zone} has the offset "${parts.timeZoneName}", which the test cannot read`);
  return {
    date: `${parts.year}-${parts.month}-${parts.day}`,
    clock: `${parts.hour}:${parts.minute}:${parts.second}`,
    offset: found[1] === undefined ? "+0000" : `${found[1]}${found[2]}${found[3]}`,
  };
}

/** A record's `time`, RFC 3339 in UTC, as microseconds since the epoch. */
export function microsOf(time) {
  const found = /^(.*?)(?:\.(\d{1,9}))?Z$/.exec(time);
  ensure(found !== null, `a record's time is "${time}", not RFC 3339 in UTC`);
  return Date.parse(`${found[1]}Z`) * 1000 + Number((found[2] ?? "").padEnd(6, "0").slice(0, 6));
}

/** The time of a record as a saved line says it: "2026-11-14 10:30:00.123456 -0500". */
export function stampOf(time, zone) {
  const micros = microsOf(time);
  const { date, clock, offset } = wallOf(Math.floor(micros / 1000), zone);
  return `${date} ${clock}.${String(micros % 1_000_000).padStart(6, "0")} ${offset}`;
}

/** The records in the order of the log: by time to the microsecond, then by boot and number. */
export const inOrder = (records) =>
  [...records].sort((a, b) => microsOf(a.time) - microsOf(b.time) || (a.boot < b.boot ? -1 : a.boot > b.boot ? 1 : 0) || a.seq - b.seq);

/** Who wrote a record, as a row says it, and as a saved line says it. */
export const fromOf = (record) => (record.from === "kernel" ? "kernel" : record.name);
export const whoOf = (record) => (record.process === null || record.process === undefined || record.from === "kernel" ? record.name : `${record.name}[${record.process}]`);

/** What `log` reads as one character of its own, escaped, as the page must for the words a person types. */
export const literal = (words) => words.replace(/[\\.[\]{}()*+?^$|]/g, "\\$&");

/** The rows a list of `records` should be, in `zone`; with `fold`, records that follow one another and say the same are one row. */
export function rowsFor(records, zone, fold) {
  const rows = [];
  for (const record of inOrder(records)) {
    const row = { time: wallOf(Math.floor(microsOf(record.time) / 1000), zone).clock, level: record.level, from: fromOf(record), text: record.text.trim(), repeated: 1 };
    const before = rows.at(-1);
    if (fold && before && before.level === row.level && before.from === row.from && before.text === row.text) before.repeated += 1;
    else rows.push(row);
  }
  return rows;
}

/** Throws unless the rows `shown` are `wanted`, naming the first that differs. */
export function sameRows(shown, wanted, what) {
  for (let at = 0; at < Math.max(shown.length, wanted.length); at += 1) {
    ensure(JSON.stringify(shown[at]) === JSON.stringify(wanted[at]), `${what}: row ${at + 1} of ${shown.length} is ${JSON.stringify(shown[at])}, and the API's ${wanted.length} lines say ${JSON.stringify(wanted[at])}`);
  }
}

/** The rows of the list of lines, which the Logs page and the view from a chart both have. */
export const rowButtons = (page) => page.locator(".loglist .lrow");

/** Waits until nothing reads: the list is not marked busy and its rows of grey bars are gone. */
export const settled = (page) =>
  page.waitForFunction(
    () => {
      const list = document.querySelector(".loglist");
      return list !== null && !list.hasAttribute("aria-busy") && document.querySelector(".lskel") === null;
    },
    undefined,
    { timeout: 30_000 },
  );

/** The rows of the list as a person reads them. */
export const shownRows = (page) =>
  page.evaluate(() =>
    [...document.querySelectorAll(".loglist .lrow")].map((row) => {
      const text = row.querySelector(".tx");
      const fold = text.querySelector(".fold");
      const words = [...text.childNodes].filter((node) => node !== fold).map((node) => node.textContent).join("");
      return {
        time: row.querySelector(".tm").textContent.trim(),
        level: row.querySelector(".sr-only").textContent.trim(),
        from: row.querySelector(".from").textContent.trim(),
        text: words.trim(),
        repeated: fold === null ? 1 : Number(/repeated (\d+) times/.exec(fold.textContent)?.[1]),
      };
    }),
  );

/** The arguments of the API's read of the same lines: what the page asks for, one page of them. */
export function argsFor({ since, until }, { priority = "debug", grep, service, kernel, lines = PAGE_LINES } = {}) {
  const args = { lines: String(lines), priority, since: String(since), until: String(until) };
  if (grep !== undefined) args.grep = grep;
  if (service !== undefined) args.service = service;
  if (kernel) args.kernel = "true";
  return args;
}

/** What the API gives for `args`, as the person's own lines or, with `all`, the whole log, in the order of the log. */
export async function readLog(page, args, all) {
  const route = `/api/1/logs${all ? "/all" : ""}?${new URLSearchParams(args)}`;
  const answer = await fetchAsPage(page, "GET", route);
  ensure(answer.status === 200 && Array.isArray(answer.json), `${route} was answered ${answer.status}: ${answer.text}`);
  return inOrder(answer.json);
}

/** Turns Show commands on, which shows each read's command under the filters. */
export async function showCommands(t, page) {
  const switchButton = page.getByRole("switch", { name: "Show commands" });
  await t.see(switchButton);
  if ((await switchButton.getAttribute("aria-checked")) !== "true") await switchButton.click();
  await page.waitForFunction(() => document.querySelector(".logs-command .inline-cmd")?.textContent.trim() !== "", undefined, { timeout: 30_000 });
}

/** The command under the filters, once it is written. */
export const commandOf = (page) => page.locator(".logs-command .inline-cmd").innerText();

/** Waits for the command to hold `words`, and gives it. */
export async function commandWith(page, words) {
  try {
    await page.waitForFunction((wanted) => wanted.every((one) => document.querySelector(".logs-command .inline-cmd")?.textContent.includes(one)), words, { timeout: 20_000 });
  } catch {
    throw new Error(`the command under the filters is "${await commandOf(page)}", which does not hold ${words.map((one) => `"${one}"`).join(" and ")}`);
  }
  return commandOf(page);
}

/** Signs `name` in and gives the account the zone, which the console asks for first. */
export async function signInAs(t, page, name, { phone = false } = {}) {
  const answer = await signIn(page, name, PASSWORD);
  ensure(answer.status() === 200, `${name} was not signed in: the console answered ${answer.status()}`);
  // The account menu is in the More sheet on a phone, so the page is signed in when the tab bar is there.
  await t.see(phone ? page.getByRole("button", { name: "More", exact: true }) : signOutButton(page));
  await leaveWelcome(t, page, ZONE);
}

/** What the banner says to someone who reads only their own lines. */
export const OWN = "You see the lines of your own programs.";

/** The targets of the phone that are under 44 px each way, which a thumb needs. */
export const smallTargets = (page) =>
  page.evaluate(() =>
    [...document.querySelectorAll("main button, main a[href], main input, main summary, main select")]
      .filter((element) => element.offsetParent !== null && !element.classList.contains("skip") && !element.classList.contains("sr-only"))
      .map((element) => ({ name: element.getAttribute("aria-label") || element.textContent.trim() || element.className, box: element.getBoundingClientRect() }))
      .filter(({ box }) => box.width > 0 && (box.height < 43.5 || box.width < 43.5))
      .map(({ name, box }) => `${name} (${Math.round(box.width)} by ${Math.round(box.height)})`),
  );
