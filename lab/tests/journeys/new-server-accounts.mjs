// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The first part of journeys/new-server, in the browser: the owner of a new
// server signs in to the console for the first time, is welcomed and skips the
// wizard, adds an administrator and a reader as the Accounts page does, and
// gives the administrator the SSH key that new-server.toml made on the lab PC.
// The steps after it sign in over SSH with that key and look at the accounts
// on the machine.

import { readFile } from "node:fs/promises";

import { ensure, fetchAsPage, lookAll, openConsole, press, signIn, signOut } from "../web-console/console.mjs";
import { eventually, folderOf, infoOf, readJson, signedIn, unlock, wordsOf } from "./journey.mjs";

const buttonOf = (scope, name) => scope.getByRole("button", { name, exact: true });
const dialogOf = (page, name) => page.getByRole("dialog", { name, exact: true });

/** The first sign-in of the account that the install made: the welcome page, Skip all keeps the zone shown, and the offer to set the machine up is put off. */
async function firstSignIn(t, info) {
  const { page } = await openConsole(t, "/");
  const answer = await signIn(page, info.owner, info.owner_password);
  ensure(answer.status() === 200, `${info.owner} was not signed in: the console answered ${answer.status()}`);
  await t.see(page.getByRole("heading", { name: /^Welcome to /, level: 1 }));
  const kept = await press(page, buttonOf(page, "Skip all"), "/api/1/account/preferences");
  ensure(kept.status() === 200, `Skip all was answered ${kept.status()}`);
  await t.see(page.getByRole("heading", { name: /^Set up .* now\?$/, level: 2 }));
  await buttonOf(page, "Later").click();
  await t.see(page.getByRole("heading", { name: "Overview", level: 1 }));
  const preferences = await readJson(await fetchAsPage(page, "GET", "/api/1/account/preferences"), "GET /api/1/account/preferences");
  ensure(preferences.zone !== "", "Skip all left the account with no time zone");
  await lookAll(t, page, "journey-first-overview");
  return page;
}

/** Add an account with the switches and passwords as a person sets them; gives the dialog that offers the account a key. */
async function addAccount(t, page, { name, full, admin, password }) {
  await page.goto(new URL("/accounts", page.url()).href);
  await t.see(page.getByRole("heading", { name: "Accounts", level: 1, exact: true }));
  await buttonOf(page, "Add an account…").click();
  const dialog = dialogOf(page, "Add an account");
  await t.see(dialog);
  await dialog.getByLabel("Name", { exact: true }).fill(name);
  await dialog.getByLabel("Full name", { exact: true }).fill(full);
  const administrator = dialog.getByRole("switch", { name: "Administrator", exact: true });
  if (admin) await administrator.check();
  else {
    await dialog.getByRole("switch", { name: "Can sign in to this console", exact: true }).check();
  }
  await dialog.getByLabel("Password", { exact: true }).fill(password);
  await dialog.getByLabel("Password, again", { exact: true }).fill(password);
  const answered = page.waitForResponse((reply) => reply.request().method() === "POST" && new URL(reply.url()).pathname === "/api/1/accounts/add");
  await buttonOf(dialog, "Add").click();
  const reply = await answered;
  ensure(reply.status() === 200, `Add an account was answered ${reply.status()}: ${await reply.text()}`);
  const offer = dialogOf(page, `Add an SSH key for ${name}`);
  await t.see(offer);
  return offer;
}

/** The account as `GET /api/1/accounts/NAME` says it, once it says it has `keys` keys. */
const accountWith = (page, name, keys) =>
  eventually(async () => {
    const shown = await readJson(await fetchAsPage(page, "GET", `/api/1/accounts/${encodeURIComponent(name)}`), `GET /api/1/accounts/${name}`);
    ensure(shown.keys.length === keys, `${name} has ${shown.keys.length} keys, not ${keys}`);
    return shown;
  }, `${name} did not have ${keys} keys`);

export default async function check(t) {
  const info = await infoOf("new-server");
  const page = await firstSignIn(t, info);
  await unlock(page, info.owner_password);

  // The administrator, with the key made on the lab PC. The dialog checks the line as authd reads it, so a key of a kind Dukovo does not take is refused first.
  const keyLine = (await readFile(`${folderOf("new-server")}/key.pub`, "utf8")).trim();
  const offer = await addAccount(t, page, { name: info.admin, full: "Journey Administrator", admin: true, password: info.admin_password });
  await offer.getByLabel("Public key", { exact: true }).fill("ssh-rsa AAAAB3NzaC1yc2E");
  await buttonOf(offer, "Add the key").click();
  await t.see(offer.getByText(/Dukovo takes Ed25519 and ECDSA keys/));
  await offer.getByLabel("Public key", { exact: true }).fill(keyLine);
  const keyAdded = page.waitForResponse((reply) => reply.request().method() === "POST" && new URL(reply.url()).pathname === `/api/1/accounts/${info.admin}/keys/add`);
  await buttonOf(offer, "Add the key").click();
  ensure((await keyAdded).status() === 200, "Add the key was not answered 200");
  await t.see(page.getByRole("heading", { name: info.admin, level: 1, exact: true }));
  const shown = await accountWith(page, info.admin, 1);
  ensure(shown.account.admin === true, `${info.admin} is not an administrator: ${JSON.stringify(shown.account)}`);
  ensure(shown.keys[0].comment === "journey-key", `the key of ${info.admin} is called ${shown.keys[0].comment}`);

  // The reader, with no key: the offer is declined.
  const second = await addAccount(t, page, { name: info.reader, full: "Journey Reader", admin: false, password: info.reader_password });
  await buttonOf(second, "Not now").click();
  await t.gone(second);

  // The list the page shows has the three accounts, and says who is an administrator.
  await page.goto(new URL("/accounts", page.url()).href);
  await t.see(page.getByRole("heading", { name: "Accounts", level: 1, exact: true }));
  for (const name of [info.owner, info.admin, info.reader]) await t.see(page.getByRole("link", { name, exact: true }).first());
  const row = (name) => page.getByRole("row").filter({ has: page.getByRole("link", { name, exact: true }) });
  await t.see(row(info.admin).getByText("Administrator", { exact: true }));
  ensure((await row(info.reader).getByText("Administrator", { exact: true }).count()) === 0, `${info.reader} is listed as an administrator`);
  await lookAll(t, page, "journey-accounts");
  await signOut(t, page);
  await page.context().close();

  // Each new account signs in to the console with the password it was given.
  for (const [name, password] of [[info.admin, info.admin_password], [info.reader, info.reader_password]]) {
    const own = await signedIn(t, name, password);
    ensure((await wordsOf(own.page.locator("body"))).length > 0, `${name}'s console page is empty`);
    await own.page.context().close();
  }
}
