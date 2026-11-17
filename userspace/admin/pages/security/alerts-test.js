// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Send a test alert from the Alerts card of Security, read when it is pressed,
// and say which channels took it.

import { testSaid } from '../alerts/test-model.js';

/** Sends a test alert to every channel from `button`, and writes what it did in `said`. `card` holds the slot for a problem. */
export async function sendTest(view, card, said, button) {
  const done = await view.api.run('alerts.test', {}, {
    button,
    where: { slot: card.errors },
    words: {
      unlock: 'send a test alert', running: 'Sending a test alert', done: 'The test alert was sent.', busy: 'Sending…',
    },
  });
  if (!done.ok) return;
  const { words, bad } = testSaid(done.result, view.time.clock(Math.floor(Date.now() / 1000)));
  said.setAttribute('class', bad ? 'small warn' : 'small ok');
  said.textContent = words;
}
