// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// lib/save-file.js against a page that is only as much of one as it needs:
// the file is the text and the name, the link is clicked once and taken out
// of the page, and the address of the text is freed after.

import assert from 'node:assert/strict';
import test from 'node:test';

/** A page with a body that takes elements, whose elements can be made, set up and clicked. */
function fakePage() {
  const events = [];
  const made = [];
  const makeElement = (tag) => {
    const attributes = new Map();
    const element = {
      tag,
      setAttribute: (name, value) => attributes.set(name, value),
      getAttribute: (name) => attributes.get(name),
      replaceChildren() {},
      click: () => events.push(['click', attributes.get('download')]),
      remove: () => events.push(['remove']),
    };
    made.push(element);
    return element;
  };
  const document = {
    createElement: makeElement,
    body: { append: (element) => events.push(['append', element.tag]) },
  };
  return { document, events, made };
}

/** Makes `document` the page's for the test, and takes it away after. */
function useDocument(context, document) {
  globalThis.document = document;
  context.after(() => { delete globalThis.document; });
}

test('importing the module does nothing and needs no page', async () => {
  const module = await import('../lib/save-file.js');
  assert.equal(typeof module.saveText, 'function');
});

test('the text is saved as a file of its name: appended, clicked and taken out once, and its address freed later', async (context) => {
  context.mock.timers.enable({ apis: ['setTimeout'] });
  const page = fakePage();
  const kept = new Map();
  const freed = [];
  useDocument(context, page.document);
  context.mock.method(URL, 'createObjectURL', (blob) => {
    const address = `blob:test/${kept.size}`;
    kept.set(address, blob);
    return address;
  });
  context.mock.method(URL, 'revokeObjectURL', (address) => freed.push(address));
  const { saveText } = await import('../lib/save-file.js');

  saveText('log-duke-nas-20261114-1030.txt', 'one\ntwo\n');

  assert.deepEqual(page.events, [['append', 'a'], ['click', 'log-duke-nas-20261114-1030.txt'], ['remove']]);
  const link = page.made[0];
  assert.equal(link.getAttribute('href'), 'blob:test/0');
  assert.equal(link.getAttribute('download'), 'log-duke-nas-20261114-1030.txt');
  const blob = kept.get('blob:test/0');
  assert.equal(blob.type, 'text/plain;charset=utf-8');
  assert.equal(await blob.text(), 'one\ntwo\n');
  // Not freed while the browser begins the save.
  assert.deepEqual(freed, []);
  context.mock.timers.tick(9999);
  assert.deepEqual(freed, []);
  context.mock.timers.tick(1);
  assert.deepEqual(freed, ['blob:test/0']);
});

test('text that is not ASCII is saved as it is, in UTF-8', async (context) => {
  const page = fakePage();
  let saved = null;
  context.mock.timers.enable({ apis: ['setTimeout'] });
  useDocument(context, page.document);
  context.mock.method(URL, 'createObjectURL', (blob) => {
    saved = blob;
    return 'blob:test/x';
  });
  context.mock.method(URL, 'revokeObjectURL', () => {});
  const { saveText } = await import('../lib/save-file.js');
  saveText('x.txt', 'příliš žluťoučký 日本語');
  assert.equal(await saved.text(), 'příliš žluťoučký 日本語');
  assert.deepEqual(new Uint8Array(await saved.slice(0, 3).arrayBuffer()), new Uint8Array([0x70, 0xc5, 0x99]));
});
