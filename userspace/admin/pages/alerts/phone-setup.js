// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Setting up a phone: on a computer, a card of steps with a code to scan; on
// the phone, which the code opens at `/alerts?phone=1`, a panel that gets
// alerts there and says how to add the console to the home screen. The page
// reads this file when it draws either; the code's own files, the card alone.

import { fill, h, icon } from '../../dom.js';
import {
  addressOf, certificateWords, fingerprintGroups, HOME_MENU_WORDS, STEPS,
} from './phone-setup-model.js';

/** What `path` answers, or null. */
const read = (view, path) => view.api.get(path).catch(() => null);

/** The card of steps for a computer. `ctx` is the page's: `{view, load, push, hostname}`. `draw()` reads the address and certificate and draws the steps. */
export function makePhoneCard(ctx) {
  const { view } = ctx;
  const body = h('div', { class: 'al-body' });
  const element = h('section', { class: 'card', id: 'phone', 'aria-labelledby': 'phone-h' }, h('h2', { id: 'phone-h', tabindex: '-1' }, 'Set up a phone'), body);

  async function draw() {
    const [about, machine, qr] = await Promise.all([
      read(view, '/api/1/console/about'),
      read(view, '/api/1/certificates/this-machine'),
      import('../../lib/qr-view.js'),
    ]);
    if (view.signal.aborted) return;
    const found = addressOf({ about, machine, hostname: ctx.hostname ?? globalThis.location?.hostname });
    if (found === null) {
      fill(body, h('p', { class: 'muted' }, 'This server has no network address that a phone could reach.'));
      return;
    }
    const picture = qr.qrView(found.url, 'QR code of this console\'s address');
    const again = h('button', { class: 'btn', type: 'button' }, 'Done on the phone: show it');
    again.addEventListener('click', () => ctx.load());
    fill(body, h('ol', { class: 'al-steps' },
      h('li', {}, STEPS.scan, h('div', { class: 'qr-row' }, picture, h('p', { class: 'cmdtext' }, found.url))),
      h('li', {}, STEPS.check, ' ', certificateWords(found.own), h('p', { class: 'cmdtext' }, fingerprintGroups(found.fingerprint).join('\n'))),
      h('li', {}, STEPS.signIn),
      h('li', {}, found.push ? STEPS.alerts : [STEPS.noPush, await ctx.push.banner()]),
      h('li', {}, STEPS.home)),
    h('div', { class: 'btns' }, again));
  }
  return { element, draw };
}

/**
 * The panel of the phone itself. `ctx` is the page's: `{test, push, install,
 * notNow}`; `install` is the browser's offer to add the console to the home
 * screen, when it made one. Gives `{element}`.
 */
export function makePhonePanel(ctx) {
  const slot = h('div', { class: 'al-errors' });
  const body = h('div', { class: 'al-body' });
  const element = h('section', { class: 'card', id: 'phone', 'aria-labelledby': 'phone-h' }, h('h2', { id: 'phone-h', tabindex: '-1' }, 'Set up this phone'), slot, body);

  function draw() {
    const on = ctx.push.isOn();
    const get = h('button', { class: 'btn default', type: 'button' }, 'Get alerts on this phone');
    get.addEventListener('click', () => ctx.push.enable(get, slot));
    const test = h('button', { class: 'btn', type: 'button' }, 'Send a test');
    test.addEventListener('click', () => ctx.test(test));
    const add = h('button', { class: 'btn', type: 'button' }, 'Add to the home screen');
    add.addEventListener('click', () => ctx.install?.prompt());
    const later = h('button', { class: 'btn', type: 'button' }, 'Not now');
    later.addEventListener('click', () => {
      ctx.notNow();
      element.hidden = true;
    });
    fill(body, h('ul', { class: 'al-steps' },
      h('li', {}, icon('check'), ' Signed in'),
      h('li', {}, on ? [icon('check'), ' This phone gets alerts. ', test] : get),
      h('li', {}, ctx.install ? add : HOME_MENU_WORDS)),
    h('div', { class: 'btns' }, later));
  }
  draw();
  ctx.push.onChange(draw);
  ctx.installed = draw;
  return { element };
}
