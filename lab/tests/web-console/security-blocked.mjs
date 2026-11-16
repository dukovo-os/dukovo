// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/security drives for the addresses blocked after wrong
// sign-ins (design 8.5), once security.toml has had a second lab address guess
// five wrong SSH passwords: the card lists the block as `firewall blocked --json`
// (`GET /api/1/security/blocked`) says it, with why and until when; Unblock
// (`firewall unblock`, `POST /api/1/security/unblock`) lets the address in at once; the
// never-block list (`firewall never-block`, `POST /api/1/security/never-block`
// and `/never-block/remove`) takes a private network without a question and asks
// before it takes one that is not private or is wider than a /16 (design 22.7).
// It runs in Chromium only, since the block is one address and the first engine
// that unblocks it ends it. Waits on S6's `firewall blocked --json` and the
// actions of the Security page.

import { ensure, lookAll, mustHaveCalled } from "./console.mjs";
import { pressAndAnswer, unlockNow } from "./trial.mjs";
import {
  buttonOf,
  cardOf,
  dialogOf,
  namesOf,
  openSecurity,
  pathOf,
  readRoute,
  ROUTES,
  signedIn,
} from "./security.mjs";

// The networks the never-block list is given: one that is private, and one that is not.
const PRIVATE_NETWORK = "192.168.77.0/24";
const OUTSIDE_NETWORK = "203.0.113.0/24";

/** The card lists the block that the attacker's guesses gave, as the machine says it. */
async function theBlock(t, page) {
  await openSecurity(t, page);
  const card = cardOf(page, "blocked");
  const answer = await readRoute(page, ROUTES.blocked);
  ensure(Array.isArray(answer.blocks) && answer.blocks.length === 1, `the machine has ${answer.blocks?.length} blocked addresses, not the attacker's one: ${JSON.stringify(answer)}`);
  const [block] = answer.blocks;
  const address = block.address.replace(/\/(32|128)$/, "");
  await t.see(card.getByRole("cell", { name: new RegExp(`^${address.replace(/\./g, "\\.")}`) }));
  await t.see(card.getByText(/wrong passwords over SSH/));
  await t.see(card.getByText(/An administrator's own address is never blocked while they are signed in\./));
  await lookAll(t, page, "security-blocked");
  return block;
}

/** Unblock asks the console to unblock the address as the firewall keeps it, and the card says that no address is blocked. */
async function unblock(t, page, block) {
  const card = cardOf(page, "blocked");
  const sent = await pressAndAnswer(t, page, buttonOf(card, "Unblock"), pathOf(ROUTES.unblock));
  ensure((sent.request().postData() ?? "").includes(block.address), `Unblock did not send the address ${block.address}`);
  await t.see(card.getByText("No address is blocked now."));
  const after = await readRoute(page, ROUTES.blocked);
  ensure(after.blocks.length === 0, `the machine still blocks ${JSON.stringify(after.blocks)} after Unblock`);
}

/** The never-block list: a private network is added at once, one outside the private ranges is asked about first, and each is removed again. */
async function neverBlock(t, page) {
  const card = cardOf(page, "blocked");
  await buttonOf(card, "change").click();
  const dialog = dialogOf(page, "Never blocked");
  await t.see(dialog);
  const field = dialog.getByLabel("Add a network", { exact: true });

  await field.fill(PRIVATE_NETWORK);
  await pressAndAnswer(t, page, buttonOf(dialog, "Add to the list"), pathOf(ROUTES.never));
  await t.see(dialog.getByText(PRIVATE_NETWORK, { exact: true }));

  // A network that is not private is asked about, and Go back sends nothing.
  await field.fill(OUTSIDE_NETWORK);
  await buttonOf(dialog, "Add to the list").click();
  const question = dialogOf(page, "Never block this network?");
  await t.see(question);
  await t.see(question.getByText(`Addresses in ${OUTSIDE_NETWORK} are never blocked, however often they get a password wrong.`));
  await buttonOf(question, "Go back").click();
  await t.gone(question);
  ensure(!(await readRoute(page, ROUTES.blocked)).never.includes(OUTSIDE_NETWORK), "Go back added the network");
  await buttonOf(dialog, "Add to the list").click();
  await t.see(question);
  await buttonOf(question, "Never block it").click();
  await t.see(dialog.getByText(OUTSIDE_NETWORK, { exact: true }));
  ensure((await readRoute(page, ROUTES.blocked)).never.includes(OUTSIDE_NETWORK), "the machine does not list the network that was added");
  await lookAll(t, page, "security-never");

  // Text that is no network is told under the field.
  await field.fill("home");
  await buttonOf(dialog, "Add to the list").click();
  await t.see(dialog.getByText("Write an address or a network, such as 192.168.1.0/24."));

  for (const network of [PRIVATE_NETWORK, OUTSIDE_NETWORK]) {
    await pressAndAnswer(t, page, dialog.getByRole("button", { name: `Remove ${network}`, exact: true }), pathOf(ROUTES.neverRemove));
    await t.gone(dialog.getByText(network, { exact: true }));
  }
  await buttonOf(dialog, "Close").click();
  await t.gone(dialog);
  const never = (await readRoute(page, ROUTES.blocked)).never;
  ensure(!never.includes(PRIVATE_NETWORK) && !never.includes(OUTSIDE_NETWORK), `the never-block list still holds what the page added: ${JSON.stringify(never)}`);
}

export default async function check(t) {
  const { page, seen } = await signedIn(t, namesOf(t.engine).admin);
  await unlockNow(page);
  const block = await theBlock(t, page);
  await unblock(t, page, block);
  await neverBlock(t, page);
  mustHaveCalled(seen, [ROUTES.blocked, ROUTES.unblock, ROUTES.never, ROUTES.neverRemove]);
  await page.context().close();
}
