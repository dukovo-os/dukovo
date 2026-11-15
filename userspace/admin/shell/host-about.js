// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The host's popover (design 4.3): the addresses the console answers on and
// the certificate's fingerprint, each with a Copy button. It opens from the
// host button, closes on Esc or a press outside, and gives focus back to the
// button. The header reads this file when the button is first pressed.

import { fill, h, icon } from '../dom.js';
import { COPIED, copyText, PRESS_CTRL_C } from '../lib/copy.js';

/** A line of the popover: its words, a value and a Copy button. */
function copyRow(words, value, say) {
  const shown = h('span', { class: 'mono value' }, value);
  const button = h('button', { class: 'btn small', type: 'button', 'aria-label': `Copy ${words.toLowerCase()}` }, icon('copy'), 'Copy');
  button.addEventListener('click', async () => say((await copyText(value, shown)) ? COPIED : PRESS_CTRL_C));
  return [h('dt', {}, words), h('dd', {}, shown, ' ', button)];
}

/**
 * Reads the server's words about itself into `popover`. Gives true when it
 * has them, and false when the server did not answer, so that the next
 * opening asks again.
 */
async function readAbout({ popover, host, get }) {
  fill(popover, h('p', { class: 'small muted' }, 'Reading…'));
  let about;
  try {
    about = await get('/api/1/console/about');
  } catch {
    // A server older than the console's about page still tells its fingerprint.
    try {
      about = { ...(await get('/api/1/fingerprint')), addresses: [location.hostname], port: Number(location.port) || 443 };
    } catch {
      fill(popover, h('p', { class: 'small muted' }, 'The server did not answer.'));
      return false;
    }
  }
  const status = h('span', { class: 'small', role: 'status' });
  const say = (words) => { status.textContent = words; };
  fill(popover,
    h('h2', {}, about.host ?? host),
    h('dl', { class: 'rows' },
      (about.addresses ?? []).map((address) => copyRow('Address', `${address}:${about.port}`, say)),
      about.fingerprint && copyRow('Certificate fingerprint', about.fingerprint, say)),
    status);
  return true;
}

/** Makes the popover in `element`, which holds `button`, and gives `{toggle}`. */
export function aboutPopover({ element, button, id, host, get }) {
  const popover = h('div', { id, class: 'popover', role: 'dialog', 'aria-label': `About ${host}`, hidden: true });
  element.append(popover);
  let loaded = false;

  function close(focus = true) {
    if (popover.hidden) return;
    popover.hidden = true;
    button.setAttribute('aria-expanded', 'false');
    document.removeEventListener('pointerdown', outside, true);
    if (focus) button.focus();
  }
  function outside(event) {
    if (!element.contains(event.target)) close(false);
  }
  popover.addEventListener('keydown', (event) => {
    if (event.key === 'Escape') {
      event.stopPropagation();
      close();
    }
  });

  return {
    async toggle() {
      if (!popover.hidden) {
        close();
        return;
      }
      popover.hidden = false;
      button.setAttribute('aria-expanded', 'true');
      document.addEventListener('pointerdown', outside, true);
      if (!loaded) {
        loaded = true;
        loaded = await readAbout({ popover, host, get });
      }
    },
  };
}
