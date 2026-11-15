// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// lib/shown-once.js (design 3.5 and 3.6, ADR 0561): the dialog of a secret
// that was answered once. What it shows and offers, what Escape asks, that a
// page that is left closes it, and that nothing of the secret is left in it
// when it closes.

import assert from 'node:assert/strict';
import test from 'node:test';

import { usePage } from './fake-dom.mjs';

const KEY = 'AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQS3QDY0';

const tick = () => new Promise((resolve) => setImmediate(resolve));

/** A page, and the view of a route on it that can be left. */
function setup(context, options) {
  const page = usePage(context, options);
  const leaving = new AbortController();
  return { ...page, leaving, view: { signal: leaving.signal } };
}

/** The dialogs that are open on `document`. */
const dialogsOf = (document) => document.body.querySelectorAll('dialog').filter((dialog) => dialog.open);

/** Shows `text` as a recovery key with `more`, and gives the promise, with the dialog once it is open. */
async function opened(context, more = {}) {
  const page = setup(context, more.page);
  const { showOnce } = await import('../lib/shown-once.js');
  const closed = showOnce(page.view, {
    title: 'Recovery key for nas', label: 'Recovery key', text: KEY, file: 'nas-recovery.key', ...more.options,
  });
  for (let waited = 0; dialogsOf(page.document).length === 0 && waited < 20; waited += 1) await tick();
  const [dialog] = dialogsOf(page.document);
  return { ...page, closed, dialog };
}

/** The button of `dialog` that says `words`. */
const buttonOf = (dialog, words) => dialog.querySelectorAll('button').find((button) => button.textContent.trim() === words);

/** Every attribute and text under `node`, for what a secret must not be left in. */
const holdings = (node) => [node, ...node.descendants].flatMap((one) => [...one.attributes.values(), one.textContent]);

test('importing the module does nothing and needs no page', async () => {
  assert.equal(typeof globalThis.document, 'undefined');
  assert.equal(typeof (await import('../lib/shown-once.js')).showOnce, 'function');
});

test('the dialog is named by its heading, which has the focus, and shows the text under its label', async (context) => {
  const { document, dialog } = await opened(context, { options: { warn: 'Keep it somewhere other than this machine.' } });
  const heading = dialog.querySelector('h2');
  assert.equal(heading.textContent, 'Recovery key for nas');
  assert.equal(dialog.getAttribute('aria-labelledby'), heading.id);
  assert.equal(document.activeElement, heading);
  const group = dialog.querySelector('[role="group"]');
  assert.equal(group.getAttribute('aria-labelledby'), group.querySelector('b').id);
  assert.equal(group.querySelector('b').textContent, 'Recovery key');
  assert.equal(group.querySelector('pre').textContent, KEY);
  assert.equal(group.querySelector('pre').getAttribute('tabindex'), '0');
  assert.match(dialog.querySelector('.banner.warn').textContent, /Keep it somewhere other than this machine\./);
});

test('Copy puts the text on the clipboard and says so in a polite region, and says what to do when the browser refuses', async (context) => {
  const { dialog, copied } = await opened(context);
  const status = dialog.querySelector('[role="status"]');
  buttonOf(dialog, 'Copy').click();
  await tick();
  assert.deepEqual(copied, [KEY]);
  assert.equal(status.textContent, 'Copied.');

  const refused = await opened(context, { page: { refuses: true } });
  buttonOf(refused.dialog, 'Copy').click();
  await tick();
  assert.equal(refused.dialog.querySelector('[role="status"]').textContent, 'Press Ctrl+C to copy.');
  assert.equal(refused.selected(), refused.dialog.querySelector('pre'), 'the text is selected for Ctrl+C');
});

test('Save as file saves the text under the name it was given, and there is no button when there is no name', async (context) => {
  context.mock.timers.enable({ apis: ['setTimeout'] });
  const saved = [];
  context.mock.method(URL, 'createObjectURL', (blob) => { saved.push(blob); return 'blob:test/0'; });
  context.mock.method(URL, 'revokeObjectURL', () => {});
  const { dialog, document } = await opened(context);
  const link = () => document.body.querySelector('a[download]');
  const clicks = [];
  const make = document.createElement;
  document.createElement = (tag) => {
    const element = make(tag);
    if (tag === 'a') element.click = () => clicks.push(element.getAttribute('download'));
    return element;
  };
  buttonOf(dialog, 'Save as file').click();
  assert.deepEqual(clicks, ['nas-recovery.key']);
  assert.equal(link(), null, 'the link is taken out of the page');
  assert.equal(await saved[0].text(), KEY);
  assert.equal(dialog.querySelector('[role="status"]').textContent, 'Saved as nas-recovery.key.');

  const without = await opened(context, { options: { file: null } });
  assert.equal(buttonOf(without.dialog, 'Save as file'), undefined);
});

test('I have kept it closes the dialog, says that it was kept, and leaves none of the text behind', async (context) => {
  const { dialog, closed, document } = await opened(context);
  buttonOf(dialog, 'I have kept it').click();
  assert.equal(await closed, true);
  assert.equal(dialogsOf(document).length, 0);
  assert.equal(holdings(dialog).some((text) => text.includes('AGE-SECRET')), false, 'the text is dropped when the dialog closes');
});

test('Escape asks before it closes, and Go back keeps the dialog open with the text', async (context) => {
  const { dialog, document, closed } = await opened(context, { options: { loss: 'You can ask for it again later.' } });
  const event = dialog.fire('cancel');
  assert.equal(event.defaultPrevented, true, 'the browser does not close it');
  const [first, question] = dialogsOf(document);
  assert.equal(first, dialog);
  assert.equal(question.querySelector('h2').textContent, 'Close without keeping it?');
  assert.match(question.textContent, /You can ask for it again later\./);
  buttonOf(question, 'Go back').click();
  await tick();
  assert.equal(dialog.open, true);
  assert.equal(dialog.querySelector('pre').textContent, KEY);

  dialog.fire('cancel');
  buttonOf(dialogsOf(document)[1], 'Close without keeping it').click();
  assert.equal(await closed, false);
  assert.equal(holdings(dialog).some((text) => text.includes('AGE-SECRET')), false);
});

test('a second Escape while the question is open does not ask again', async (context) => {
  const { dialog, document } = await opened(context);
  dialog.fire('cancel');
  dialog.fire('cancel');
  assert.equal(dialogsOf(document).length, 2);
});

test('a page that is left closes the dialog without keeping, and drops the text', async (context) => {
  const { dialog, closed, leaving } = await opened(context);
  leaving.abort();
  assert.equal(await closed, false);
  assert.equal(holdings(dialog).some((text) => text.includes('AGE-SECRET')), false);
});

test('a page that was left before the dialog could open shows nothing', async (context) => {
  const page = setup(context);
  page.leaving.abort();
  const { showOnce } = await import('../lib/shown-once.js');
  assert.equal(await showOnce(page.view, { title: 'x', text: KEY }), false);
  assert.equal(dialogsOf(page.document).length, 0);
});

test('with a QR code the picture has a label that points to the text, which is behind a button that opens it', async (context) => {
  const settings = '[Interface]\nPrivateKey = MARKER\nAddress = 10.0.0.2/32\n';
  const { dialog } = await opened(context, { options: { label: 'Settings', text: settings, file: 'phone.conf', qr: true } });
  const picture = dialog.querySelector('.qr-row svg');
  assert.equal(picture.getAttribute('role'), 'img');
  assert.match(picture.getAttribute('aria-label'), /^QR code of the settings\. The same text is under Show the settings as text\.$/);
  const pre = dialog.querySelector('pre');
  const reveal = buttonOf(dialog, 'Show the settings as text');
  assert.equal(pre.hidden, true);
  assert.equal(reveal.getAttribute('aria-expanded'), 'false');
  reveal.click();
  assert.equal(pre.hidden, false);
  assert.equal(reveal.getAttribute('aria-expanded'), 'true');
  assert.equal(pre.textContent, settings);
});

test('a text too long for a QR code is shown as text, with the way to keep it', async (context) => {
  const { dialog } = await opened(context, { options: { label: 'Settings', text: 'x'.repeat(400), qr: true } });
  assert.equal(dialog.querySelector('svg'), null);
  assert.equal(dialog.querySelector('pre').hidden, false);
  assert.match(dialog.textContent, /It is too long for a QR code\. Copy it or save it as a file\./);
  assert.equal(buttonOf(dialog, 'Show the settings as text'), undefined);
});
