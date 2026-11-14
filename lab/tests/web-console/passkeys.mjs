// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What web-console/passkeys drives in each engine, with the lab's browser
// runner (lab repository, docs/test-files.md, "Browser tests"): passkeys
// in the console, which need the page opened by the server's name, so
// the page is opened as `localhost` where the lab reaches the machine at
// 127.0.0.1, which is how a QEMU machine is reached.
//
// Chromium has a virtual authenticator, which its DevTools protocol adds
// to a page: an administrator adds a passkey to the account, renames it,
// signs in with it alone, unlocks the console with it and removes it. The
// relying party of the passkey is `localhost`. Firefox and WebKit have no
// virtual authenticator, so they check that the page offers the passkey
// button, which the address page does not, and stop there.
//
// Chromium may refuse WebAuthn on a page whose certificate errors it was
// told to ignore, which is how the lab opens every page, and the console's
// certificate is the machine's own. The script asks the page whether it
// may make a passkey; if it may not, it opens the page in a Chromium of
// its own, started with the console's public key trusted, as a person who
// compared the fingerprint with the one at the machine has trusted it. The
// console does nothing differently for either, and the lab's log says
// which was needed.

import {
  SECOND_STEP,
  answerTo,
  ensure,
  isAddress,
  look,
  mustHaveCalled,
  openConsole,
  openSecondStep,
  pinOf,
  press,
  recoveryCodesShown,
  servedCertificate,
  sessionOf,
  signInDone,
  signInHeading,
  signOut,
  signOutButton,
  track,
  watchPolicy,
} from "./console.mjs";

// The password that passkeys.toml gives the accounts it makes.
const PASSWORD = "lab-console-password";

// The passkey's name, and the name it is renamed to.
const KEY = "lab key";
const RENAMED = "lab key renamed";

// The calls that lab/coverage.toml says this test covers, which the
// browser must have made by the end.
const ROUTES = [
  "POST /api/1/sign-in/passkey/options",
  "POST /api/1/sign-in/passkey",
  "GET /api/1/account/second-step",
  "POST /api/1/account/second-step/passkey/begin",
  "POST /api/1/account/second-step/passkey/add",
  "POST /api/1/account/second-step/passkey/rename",
  "POST /api/1/account/second-step/passkey/remove",
  "POST /api/1/unlock",
  "POST /api/1/unlock/passkey/options",
];

// What a Chromium page opened here looks like, as the lab's runner opens one.
const PAGE = { viewport: { width: 1280, height: 800 }, locale: "en-US", ignoreHTTPSErrors: true };

const PASSKEY_BUTTON = "Sign in with a passkey";

/** The header's button called `name`. */
const headerButton = (page, name) => page.getByRole("banner").getByRole("button", { name, exact: true });

/** The sign-in page of an address offers no passkey, and says why. */
async function byAddress(t, page) {
  await t.see(signInHeading(page));
  const button = page.getByRole("button", { name: PASSKEY_BUTTON });
  await t.see(button);
  ensure(await button.isDisabled(), "the passkey button is enabled on a page opened by an address");
  await t.see(page.getByText("not by its address"));
  await look(t, page, "by-address");
}

/** A virtual authenticator that holds resident keys and verifies the person, in `page`'s Chromium. */
async function addAuthenticator(page) {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("WebAuthn.enable");
  const { authenticatorId } = await cdp.send("WebAuthn.addVirtualAuthenticator", {
    options: {
      protocol: "ctap2",
      transport: "internal",
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
    },
  });
  return { cdp, authenticatorId };
}

/**
 * Why the browser would not make a passkey on `page`, as the error it
 * raises, or null when it makes one, which is then taken away again.
 */
async function refusalOf(page, { cdp, authenticatorId }) {
  const refusal = await page.evaluate(async () => {
    try {
      await navigator.credentials.create({
        publicKey: {
          challenge: new Uint8Array(32),
          rp: { name: "probe" },
          user: { id: new Uint8Array(8), name: "probe", displayName: "probe" },
          pubKeyCredParams: [{ type: "public-key", alg: -7 }],
          authenticatorSelection: { userVerification: "required" },
        },
      });
      return null;
    } catch (error) {
      return `${error.name}: ${error.message}`;
    }
  });
  if (refusal === null) await cdp.send("WebAuthn.clearCredentials", { authenticatorId });
  return refusal;
}

/**
 * The console opened at `url`, by its name, in a Chromium that allows
 * passkeys on it. `first` is a page of the same browser. Gives the page,
 * the calls it goes on to make, and what to call when the test is done.
 */
async function chromiumByName(t, url, first) {
  let page = await t.open(url);
  let refusal = await refusalOf(page, await addAuthenticator(page));
  let close = async () => {};
  if (refusal !== null) {
    ensure(/certificate/i.test(refusal), `Chromium would not make a passkey on ${url}: ${refusal}`);
    console.log(`Chromium: ${refusal}; opening the page in a Chromium that trusts the console's key`);
    await page.context().close();
    const pin = pinOf(await servedCertificate(url));
    const browser = await first.context().browser().browserType().launch({
      args: [`--ignore-certificate-errors-spki-list=${pin}`],
    });
    close = () => browser.close();
    page = await (await browser.newContext(PAGE)).newPage();
    await page.goto(url, { waitUntil: "load" });
    refusal = await refusalOf(page, await addAuthenticator(page));
    ensure(refusal === null, `Chromium would not make a passkey on ${url}, with the console's key trusted: ${refusal}`);
  }
  const seen = track(page);
  await watchPolicy(page);
  // The page's first calls came before anything listened for them. The
  // page keeps its authenticator through a reload.
  await page.reload();
  return { page, seen, close };
}

/** Adds the passkey on the second-step page, with the password, and keeps the recovery codes it brings. */
async function addPasskey(t, page) {
  await openSecondStep(t, page);
  await t.see(page.getByText("0 passkeys", { exact: true }));
  await look(t, page, "second-step");

  const adder = page.getByRole("button", { name: "Add a passkey" });
  ensure(await adder.isEnabled(), "the button that adds a passkey is off on a page opened by a name");
  await adder.click();
  const form = page.getByRole("form", { name: "Add a passkey" });
  await form.getByLabel("Name", { exact: true }).fill(KEY);
  await form.getByLabel("Your password", { exact: true }).fill(PASSWORD);
  await look(t, page, "add-passkey");

  const added = answerTo(page, `${SECOND_STEP}/passkey/add`);
  const begun = await press(page, form.getByRole("button", { name: "Add the passkey" }), `${SECOND_STEP}/passkey/begin`);
  ensure(begun.status() === 200, `the console answered ${begun.status()} to the passkey's beginning`);
  const answer = await added;
  ensure(answer.status() === 200, `the console answered ${answer.status()} to the new passkey`);

  // The first second step brings the recovery codes.
  await t.see(page.getByRole("heading", { name: "Your recovery codes" }));
  await recoveryCodesShown(page);
  await page.getByRole("button", { name: "I have saved them" }).click();
  await t.see(page.getByText("1 passkey", { exact: true }));
  await t.see(page.getByRole("button", { name: `Rename ${KEY}` }));
  await t.see(page.getByText(new URL(page.url()).hostname, { exact: true }));
  await look(t, page, "passkey-added");
}

/** The passkey's name changes. */
async function renamePasskey(t, page) {
  await page.getByRole("button", { name: `Rename ${KEY}` }).click();
  const form = page.getByRole("form", { name: `Rename ${KEY}` });
  await form.getByLabel("Name", { exact: true }).fill(RENAMED);
  const renamed = await press(page, form.getByRole("button", { name: "Rename", exact: true }), `${SECOND_STEP}/passkey/rename`);
  ensure(renamed.status() === 200, `the console answered ${renamed.status()} to the new name`);
  await t.see(page.getByRole("button", { name: `Remove ${RENAMED}` }));
}

/** The unlock dialog offers the passkey just added, with no page load between. */
async function unlockOffersPasskey(t, page) {
  await headerButton(page, "Unlock").click();
  const dialog = page.getByRole("dialog", { name: "Unlock to make changes" });
  await t.see(dialog.getByRole("button", { name: "Unlock with your passkey" }));
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await t.gone(dialog);
}

/** The passkey signs in without a name or a password. */
async function signInAlone(t, page, name) {
  await signOut(t, page);
  await t.see(page.getByRole("button", { name: PASSKEY_BUTTON }));
  await look(t, page, "sign-in");

  const asked = answerTo(page, "/api/1/sign-in/passkey/options");
  const signedIn = await press(page, page.getByRole("button", { name: PASSKEY_BUTTON }), "/api/1/sign-in/passkey");
  const options = await asked;
  ensure(options.status() === 200, `the console answered ${options.status()} to the passkey's challenge`);
  ensure(signedIn.status() === 200, `the console answered ${signedIn.status()} to the passkey`);
  await t.see(signOutButton(page));
  const session = await sessionOf(page);
  ensure(session.name === name, `the passkey signed in ${session.name}, not ${name}`);
  ensure(session.kinds.includes("passkey"), `the session offers ${session.kinds} for an unlock, with a passkey`);
}

/** The header's "Unlock" unlocks with the passkey, which the dialog offers first. */
async function unlockWithPasskey(t, page) {
  await headerButton(page, "Unlock").click();
  const dialog = page.getByRole("dialog", { name: "Unlock to make changes" });
  const submit = dialog.getByRole("button", { name: "Unlock with your passkey" });
  await t.see(submit);
  await look(t, page, "unlock-with-passkey");

  const asked = answerTo(page, "/api/1/unlock/passkey/options");
  const unlocked = await press(page, submit, "/api/1/unlock");
  const options = await asked;
  ensure(options.status() === 200, `the console answered ${options.status()} to the passkey's challenge`);
  ensure(unlocked.status() === 200, `the console answered ${unlocked.status()} to the passkey`);
  await t.gone(dialog);
  await t.see(page.getByText("15 min", { exact: true }));
}

/** Removes the passkey: the unlocked console asks for no password. */
async function removePasskey(t, page) {
  await openSecondStep(t, page);
  await page.getByRole("button", { name: `Remove ${RENAMED}` }).click();
  const form = page.getByRole("form", { name: `Remove ${RENAMED}` });
  await t.gone(form.getByLabel("Your password", { exact: true }));
  const removed = await press(page, form.getByRole("button", { name: "Remove the passkey" }), `${SECOND_STEP}/passkey/remove`);
  ensure(removed.status() === 200, `the console answered ${removed.status()} to the passkey's removal`);
  await t.see(page.getByText("0 passkeys", { exact: true }));
  await look(t, page, "passkey-removed");
}

export default async function check(t) {
  const name = `wc-passkeys-${t.engine}`;
  const here = new URL(t.url);
  const { page: address } = await openConsole(t, "/");

  // A passkey is for a name, and the lab reaches a QEMU machine by an
  // address, which the machine itself knows as `localhost`.
  let url = t.url;
  if (isAddress(here.hostname)) {
    await byAddress(t, address);
    if (!["127.0.0.1", "[::1]"].includes(here.hostname)) {
      t.skip("passkeys need the console opened by a name, and this machine is reached by an address only");
    }
    url = `https://localhost:${here.port}`;
  }

  if (t.engine !== "chromium") {
    const { page } = await openConsole(t, url);
    await t.see(signInHeading(page));
    const button = page.getByRole("button", { name: PASSKEY_BUTTON });
    await t.see(button);
    ensure(await button.isEnabled(), "the passkey button is off on a page opened by a name");
    await look(t, page, "by-name");
    t.skip("no virtual authenticator in this engine");
  }

  const { page, seen, close } = await chromiumByName(t, url, address);
  try {
    await signInDone(t, page, name, PASSWORD);
    await addPasskey(t, page);
    await renamePasskey(t, page);
    await unlockOffersPasskey(t, page);
    await signInAlone(t, page, name);
    await unlockWithPasskey(t, page);
    await removePasskey(t, page);
    await signOut(t, page);
    mustHaveCalled(seen, ROUTES);
  } finally {
    await close();
  }
}
