// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The browsers of journeys/day-in-the-life: two people at the console for as
// many minutes as the journey's facts say (`minutes` in the file that
// day-in-the-life.toml writes, so one number sets the length of the browsers
// and of the SSH session beside them). An administrator, in the runner's own
// engine, and a reader, in the other engine of the browser kit when it has
// one, each go round the pages of the console, a few at a time, and each round
// change a setting of their own account and put it back; every fourth round the
// administrator tries a firewall rule and takes it back with Undo now. Every
// page keeps its heading, the browser's own errors and the content security
// policy's stops are held to none, and each setting is what it was after
// being put back. The steps after look at the machine.

import { ensure, fetchAsPage, mustBe, policyStopped, sessionOf, signInDone, sleep, watchPolicy } from "../web-console/console.mjs";
import { bannerBecomes, bannerEnds, pressAndAnswer, undoOf } from "../web-console/trial.mjs";
import { journeyInfo, otherEngine, readJson, unlock } from "./journey.mjs";

// The pages an administrator goes round, and the pages a reader does: the path and the heading each shows.
const ADMIN_PAGES = [
  ["/", "Overview"], ["/logs", "Logs"], ["/services", "Services"], ["/storage", "Storage"], ["/networking", "Networking"],
  ["/hardware", "Hardware"], ["/snapshots", "Snapshots and backups"], ["/alerts", "Alerts"], ["/accounts", "Accounts"],
  ["/updates", "Updates"], ["/reliability", "Reliability"], ["/security", "Security"], ["/certificates", "Certificates"],
  ["/apps", "Apps"], ["/firewall", "Firewall"],
];
const READER_PAGES = [["/", "Overview"], ["/logs", "Logs"], ["/services", "Services"], ["/storage", "Storage"], ["/account", "Your account"], ["/firewall", "Firewall"], ["/alerts", "Alerts"]];

// How many pages a round shows, and how long a person rests between rounds.
const PAGES_A_ROUND = 3;
const REST_MS = 6000;

// The zone each person puts in and takes out.
const OTHER_ZONE = "Europe/Prague";

const TRIED = "The firewall rules are being tried.";

/** The page of a person who signed in at the console, with what its browser said of its own faults. */
async function person(opening, name, password) {
  const page = await opening.open("/");
  const faults = [];
  page.on("pageerror", (error) => faults.push(`${name}: ${error.message}`));
  await watchPolicy(page);
  await signInDone({ see: (locator) => locator.waitFor({ state: "visible", timeout: 30_000 }) }, page, name, password);
  const session = await sessionOf(page);
  ensure(session.name === name, `the session is ${session.name}'s, not ${name}'s`);
  return { page, faults, name, rounds: 0 };
}

/** Shows `count` pages of `pages` from the place `round` has reached, each with its heading. */
async function readPages(who, pages, round) {
  for (let one = 0; one < PAGES_A_ROUND; one += 1) {
    const [path, heading] = pages[(round * PAGES_A_ROUND + one) % pages.length];
    await who.page.goto(new URL(path, who.page.url()).href);
    await who.page.getByRole("heading", { name: heading, level: 1, exact: true }).waitFor({ state: "visible", timeout: 30_000 });
    await sleep(1000);
    const stopped = await policyStopped(who.page);
    ensure(stopped.length === 0, `${who.name}: the content security policy stopped ${stopped.join(", ")} on ${path}`);
  }
}

/** The account's own time zone is changed and put back, and each read says what was set. */
async function changeAndPutBack(who) {
  const was = await readJson(await fetchAsPage(who.page, "GET", "/api/1/account/preferences"), "GET /api/1/account/preferences");
  const to = was.zone === OTHER_ZONE ? "UTC" : OTHER_ZONE;
  const set = await fetchAsPage(who.page, "POST", "/api/1/account/preferences", { zone: to });
  ensure(set.status === 200, `${who.name} changing the zone to ${to} was answered ${set.status}: ${set.text}`);
  ensure((await readJson(await fetchAsPage(who.page, "GET", "/api/1/account/preferences"), "GET /api/1/account/preferences")).zone === to, `${who.name}'s zone is not ${to} after it was set`);
  const back = await fetchAsPage(who.page, "POST", "/api/1/account/preferences", { zone: was.zone });
  ensure(back.status === 200, `${who.name} putting the zone back to ${was.zone} was answered ${back.status}: ${back.text}`);
  ensure((await readJson(await fetchAsPage(who.page, "GET", "/api/1/account/preferences"), "GET /api/1/account/preferences")).zone === was.zone, `${who.name}'s zone is not ${was.zone} after it was put back`);
}

/** A firewall rule is tried and taken back at once, and the rules are as they were. */
async function tryAndUndo(t, who, password) {
  const { page } = who;
  const rulesNow = async () => (await readJson(await fetchAsPage(page, "GET", "/api/1/firewall"), "GET /api/1/firewall")).rules.map((rule) => rule.text);
  const before = await rulesNow();
  const session = await sessionOf(page);
  await unlock(page, password);
  await page.goto(new URL("/firewall", page.url()).href);
  await page.getByRole("heading", { name: "Firewall", level: 1, exact: true }).waitFor({ state: "visible", timeout: 30_000 });
  await page.getByRole("button", { name: "Add a rule", exact: true }).click();
  const form = page.locator("form#add");
  await form.waitFor({ state: "visible", timeout: 15_000 });
  await form.getByRole("radio", { name: "Allow", exact: true }).check();
  await form.locator("#fw-protocol").selectOption("tcp");
  await page.locator("#fw-ports").fill("8090");
  await form.getByRole("radio", { name: "An address or network…", exact: true }).check();
  await page.locator("#fw-from-text").fill(session.from);
  await pressAndAnswer(t, page, form.getByRole("button", { name: /^Try for / }), "/api/1/firewall/add");
  await bannerBecomes(page, (banner) => banner.running && banner.heading === TRIED, "show the rules being tried");
  ensure((await rulesNow()).length === before.length + 1 || (await rulesNow()).length === before.length, "the rules are neither as they were nor with the new rule while it is tried");
  await pressAndAnswer(t, page, undoOf(page), "/api/1/firewall/trial/undo");
  await bannerEnds(page, "undone", 20);
  const until = Date.now() + 30_000;
  for (;;) {
    const now = await rulesNow();
    if (JSON.stringify(now) === JSON.stringify(before)) break;
    ensure(Date.now() < until, `the rules are ${JSON.stringify(now)} after Undo now, and were ${JSON.stringify(before)}`);
    await sleep(1000);
  }
}

/** One person's rounds until `deadline`: pages, a setting changed and put back, and for the administrator a firewall trial every fourth round. */
async function rounds(t, who, pages, deadline, { admin, password }) {
  for (let round = 0; Date.now() < deadline; round += 1) {
    if (admin) await unlock(who.page, password);
    await readPages(who, pages, round);
    await changeAndPutBack(who);
    if (admin && round % 4 === 3) await tryAndUndo(t, who, password);
    if (!admin) {
      const answer = await fetchAsPage(who.page, "POST", "/api/1/firewall/add", { action: "allow", protocol: "tcp", ports: "8091" });
      mustBe(answer, 403, "ADM-122", `${who.name}'s try at a firewall rule in round ${round}`);
    }
    who.rounds += 1;
    ensure(who.faults.length === 0, who.faults.join("; "));
    await sleep(REST_MS);
  }
}

export default async function check(t) {
  const info = await journeyInfo();
  const minutes = Number(info.minutes);
  ensure(Number.isFinite(minutes) && minutes > 0, `the journey's facts give ${info.minutes} minutes`);
  const deadline = Date.now() + minutes * 60_000;

  const second = await otherEngine(t);
  console.log(second === null ? `the reader is in a second ${t.engine} browser: the kit has no other engine` : `the administrator is in ${t.engine} and the reader in ${second.engine}`);
  const adminSide = { open: (path) => t.open(path), engine: t.engine };
  const readerSide = second ?? adminSide;

  const admin = await person(adminSide, info.ada, info.ada_password);
  const reader = await person(readerSide, info.ben, info.ben_password);
  console.log(`day-in-the-life: ${minutes} minutes, ${info.ada} in ${t.engine}, ${info.ben} in ${readerSide.engine}`);

  const finished = await Promise.allSettled([
    rounds(t, admin, ADMIN_PAGES, deadline, { admin: true, password: info.ada_password }),
    rounds(t, reader, READER_PAGES, deadline, { admin: false, password: info.ben_password }),
  ]);
  if (second !== null) await second.close();
  const failed = finished.find((one) => one.status === "rejected");
  if (failed) throw failed.reason;
  console.log(`day-in-the-life: ${info.ada} went ${admin.rounds} rounds and ${info.ben} ${reader.rounds}`);
  ensure(admin.rounds >= 5 && reader.rounds >= 5, `the two went ${admin.rounds} and ${reader.rounds} rounds in ${minutes} minutes`);
}
