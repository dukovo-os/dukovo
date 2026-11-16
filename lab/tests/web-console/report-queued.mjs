// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/report drives in one engine while the machine's reportd
// names a server of the lab PC's where nothing listens (report.toml writes
// /system/settings/report.toml so and restarts reportd, which reads it only
// then): a report sent from the page waits. The page asks reportd for a draft
// of "Something else" with one answer typed, and Send (`POST
// /api/1/reports/send`) is answered with a report that is queued, not sent.
// The banner under the buttons says that it is queued, how reportd tries
// again, and why it waits in reportd's own words, with Retry now. The table of
// Reports from this server (`GET /api/1/reports`) has the report as Queued,
// with how many times it was tried and when it is tried next, and its own
// Retry now.
//
// Retry now of the banner (`POST /api/1/reports/retry`) has reportd try every
// report that waits now. The server is still away, so the report waits on: the
// banner gives way to the words that the reports that wait were tried again,
// the table says it was tried once more, and the machine lists it as queued.
// report.toml then names the staging server again, and report-retry.mjs
// presses the table's Retry now, since the banner lives only in the page that
// pressed Send.
//
// The look of the page with its banner is checked in each way the console can
// look (console.mjs's lookAll). The account is the administrator that
// report.toml made, so no script needs the lab's password.

import { ensure, fetchAsPage, lookAll, mustHaveCalled } from "./console.mjs";
import { pressAndAnswer, unlockNow } from "./trial.mjs";
import { card, choiceOf, draftShown, openReport, questions, signedIn } from "./report.mjs";

const ADMIN = "wc-report";

/** What the page types, which the table calls the report by, and which the staging server's copy of it holds. */
export const WORDS = "The page sent this report while the server was away.";

export const SEND = "/api/1/reports/send";
export const RETRY = "/api/1/reports/retry";
export const LIST = "/api/1/reports";

// The words the page says where this script looks for them.
const QUEUED = /^Queued: it is sent when reports\.dukovo\.org can be reached\. reportd tries again after 1, 5, 15 and 60 minutes/;
const TRIED_AGAIN = "The reports that wait were tried again. The table below says where each stands.";

/** The card of the table of Reports from this server, and the row of this script's report in it. */
export const tableOf = (page) => card(page, "Reports from this server");
export const rowOf = (page) => tableOf(page).locator("tbody tr").filter({ hasText: WORDS });

/** This script's report as `GET /api/1/reports` lists it, `{state, number, tries, why, ...}`. */
export async function listed(page) {
  const answer = await fetchAsPage(page, "GET", LIST);
  const reports = Array.isArray(answer.json) ? answer.json : answer.json?.reports;
  ensure(answer.status === 200 && Array.isArray(reports), `GET ${LIST} was answered ${answer.status}: ${answer.text}`);
  const found = reports.filter((report) => report.title === WORDS);
  ensure(found.length === 1, `GET ${LIST} lists ${found.length} reports called ${JSON.stringify(WORDS)}: ${answer.text}`);
  return found[0];
}

export default async function check(t) {
  const a = await signedIn(t, ADMIN);
  const { page } = a;
  await unlockNow(page);
  await openReport(t, page);

  await choiceOf(page, "Something else").click();
  const happened = questions(page).getByLabel("What happened?", { exact: true });
  await t.see(happened);
  await happened.fill(WORDS);
  await happened.blur();
  await draftShown(t, page);
  await pressAndAnswer(t, page, page.getByRole("button", { name: "Send", exact: true }), SEND);

  // The banner says that it is queued and why, and the table says so too. The
  // one try that Send made is the only one, as reportd's own next try is a
  // minute away.
  const status = page.locator(".rp-status");
  await t.see(status.getByText(QUEUED));
  await t.see(status.getByText(/^Why: \S/));
  await t.see(status.getByRole("button", { name: "Retry now", exact: true }));
  const row = rowOf(page);
  await t.see(row);
  await t.see(row.getByText("Queued", { exact: true }));
  await t.see(row.getByText(/^Sends when reports\.dukovo\.org can be reached · tried 1 time, next at /));
  await t.see(tableOf(page).getByRole("button", { name: "Retry now", exact: true }));
  const first = await listed(page);
  ensure(first.state === "queued" && first.number == null && first.tries === 1 && typeof first.why === "string" && first.why !== "", `the machine lists the report as ${JSON.stringify(first)}, not as queued and tried once, with why`);
  await lookAll(t, page, "report-queued");

  // The server is still away: the report waits on, tried once more. The
  // looks take long enough for reportd's own try, a minute after the first,
  // to have come between, so the count is only said to have grown.
  await pressAndAnswer(t, page, status.getByRole("button", { name: "Retry now", exact: true }), RETRY);
  await t.see(status.getByText(TRIED_AGAIN, { exact: true }));
  await t.gone(status.getByText(QUEUED));
  await t.see(row.getByText(/ · tried \d+ times, next at /));
  const second = await listed(page);
  ensure(second.state === "queued" && second.number == null && second.tries > first.tries, `the machine lists the report as ${JSON.stringify(second)} after Retry now, not as queued and tried more often than the ${first.tries} before`);

  mustHaveCalled(a.seen, ["POST /api/1/reports/drafts", `POST ${SEND}`, `POST ${RETRY}`, `GET ${LIST}`]);
  await page.context().close();
}
