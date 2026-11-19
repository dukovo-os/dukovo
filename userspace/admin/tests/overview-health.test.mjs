// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Health card of the Overview (design 10.1), drawn: that each row of its
// list, and of the dialog that See all opens, says which need it is, so that
// two rows with one title can be told apart.

import assert from 'node:assert/strict';
import test from 'node:test';

import { usePage } from './fake-dom.mjs';

const TITLE = 'diskhealth.toml changed outside this console';

const need = (id, at, title = TITLE) => ({
  id, severity: 'warn', title, detail: 'Someone edited it by hand.', href: null, icon: 'info', at,
});

/** Five needs, newest first, of which the first two have one title. */
const NEEDS = [
  need('change-12', 500),
  need('change-9', 400),
  need('change-7', 300, 'nginx.toml changed outside this console'),
  need('change-5', 200, 'sshd.toml changed outside this console'),
  need('change-3', 100, 'logd.toml changed outside this console'),
];

const tick = () => new Promise((resolve) => setImmediate(resolve));

/** The view of an Overview whose parts give `needs` and whose checks are not kept. */
const viewOf = (needs) => ({
  signal: new AbortController().signal,
  session: { admin: true },
  api: { get: async () => { throw Object.assign(new Error('no result is kept'), { id: 'ADM-123' }); } },
  parts: async () => [{ value: needs }],
  time: { when: () => 'Nov 14 9:00 AM' },
});

/** The card, drawn once, on a page whose elements take the ring's custom property. */
async function drawn(context) {
  const page = usePage(context);
  const make = page.document.createElement;
  page.document.createElement = (tag) => {
    const element = make(tag);
    element.style = { setProperty() {} };
    return element;
  };
  const { makeHealth } = await import('../pages/overview/health-card.js');
  const card = makeHealth(viewOf(NEEDS));
  await card.load();
  return { ...page, element: card.element };
}

const needsOf = (rows) => rows.map((row) => row.getAttribute('data-need'));

test('each row of the list names its need, so that two rows with one title are told apart', async (context) => {
  const { element } = await drawn(context);
  const rows = element.querySelectorAll('li');
  assert.deepEqual(needsOf(rows), ['change-12', 'change-9', 'change-7', 'change-5']);
  const same = rows.filter((row) => row.textContent.includes(TITLE));
  assert.deepEqual(needsOf(same), ['change-12', 'change-9']);
});

test('each row of the See all dialog names its need as the list does', async (context) => {
  const { element, document } = await drawn(context);
  element.querySelector('#ov-see-all').click();
  for (let waited = 0; document.body.querySelector('dialog') === null && waited < 20; waited += 1) await tick();
  const dialog = document.body.querySelector('dialog');
  assert.equal(dialog !== null, true, 'See all opens a dialog');
  const rows = dialog.querySelectorAll('li.all-row');
  assert.deepEqual(needsOf(rows), NEEDS.map((one) => one.id));
  assert.equal(dialog.querySelector('[data-need="change-9"]').textContent.includes(TITLE), true);
});
