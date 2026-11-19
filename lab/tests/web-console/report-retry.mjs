// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/report drives in one engine once the machine's reportd
// names the staging server of the lab PC again (report.toml writes
// /system/settings/report.toml so and restarts reportd): the report that
// report-queued.mjs sent from the page, which waited, is sent by Retry now.
// The page is a new one, so it has no banner; the table of Reports from this
// server (`GET /api/1/reports`) has the report as Queued, with its Retry now.
//
// Retry now (`POST /api/1/reports/retry`) has reportd try every report that
// waits, and now the server answers. The report is no longer Queued, the table
// has no Retry now while nothing waits, and the machine lists the report as
// sent, with the number that the server gave it. The staging server holds the
// words that were typed, and Check the status (`GET /api/1/reports/status`)
// shows that it knows the report. Delete (`POST /api/1/reports/{number}/delete`)
// then takes the report off the server and the table, so that the steps after
// find none.
//
// The account is the administrator that report.toml made, so no script needs
// the lab's password.

import { ensure, lookAll, mustHaveCalled } from "./console.mjs";
import { pressAndAnswer, unlockNow } from "./trial.mjs";
import { deskCopy, openReport, signedIn } from "./report.mjs";
import { LIST, RETRY, WORDS, listed, rowOf, tableOf } from "./report-queued.mjs";

const ADMIN = "wc-report";

export default async function check(t) {
  const a = await signedIn(t, ADMIN);
  const { page } = a;
  await unlockNow(page);
  await openReport(t, page);

  // The report waited through the restart of reportd, as the queue is kept on disk.
  const table = tableOf(page);
  const row = rowOf(page);
  const retry = table.getByRole("button", { name: "Retry now", exact: true });
  await t.see(row);
  await t.see(row.getByText("Queued", { exact: true }));
  await t.see(retry);
  const waiting = await listed(page);
  ensure(waiting.state === "queued" && waiting.number == null, `the machine lists the report as ${JSON.stringify(waiting)}, not as queued`);
  await lookAll(t, page, "report-retry");

  // The server answers now: Retry now sends it.
  await pressAndAnswer(t, page, retry, RETRY);
  await t.gone(row.getByText("Queued", { exact: true }));
  await t.gone(retry);
  const sent = await listed(page);
  ensure(sent.state === "sent" && Number.isInteger(sent.number), `the machine lists the report as ${JSON.stringify(sent)} after Retry now, not as sent with a number`);
  const { number } = sent;

  // The staging server holds what was typed, and says that it received it.
  const copy = await deskCopy(page, number);
  ensure(copy !== null, `the staging server holds no report ${number}`);
  ensure(copy.includes(WORDS), `the staging server's copy of report ${number} does not have the words that were typed`);
  await table.getByRole("button", { name: "Check the status", exact: true }).click();
  await t.see(row.getByText("Received", { exact: true }));

  // The report is taken off the server and the table.
  const dialog = page.getByRole("dialog", { name: `Delete report ${number}?`, exact: true });
  await row.getByRole("button", { name: `Delete report ${number}`, exact: true }).click();
  await t.see(dialog);
  await pressAndAnswer(t, page, dialog.getByRole("button", { name: "Delete", exact: true }), `/api/1/reports/${number}/delete`);
  await t.gone(row);
  ensure((await deskCopy(page, number)) === null, `report ${number} is still on the staging server after Delete`);

  mustHaveCalled(a.seen, [`POST ${RETRY}`, `GET ${LIST}`, "GET /api/1/reports/status", `POST /api/1/reports/${number}/delete`]);
  await page.context().close();
}
