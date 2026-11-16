// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The fields that set up private access (design 8.8) and what setting it up
// runs: `private.serve`, then `firewall.add` for WireGuard's port. The Security
// page's card and the Overview's step of Finish setting up draw the same
// fields and run the same two actions. Read when either is opened.

import { h } from '../../dom.js';
import { textField } from './fields.js';
import {
  blankSetUp, DEFAULTS, openPort, routerWords, serveArgs, setUpProblems,
} from './private-form-model.js';

/**
 * The set-up fields, with the console's host name as the address devices reach
 * this server at. Gives `{elements, check(), fault(problems)}`: `check()` gives
 * `{args, problems}`, the arguments of `private.serve` or null and what is
 * wrong with each field, and says nothing on the page; `fault` puts what is
 * wrong under the fields and moves the focus to the first.
 */
export function setUpFields(host) {
  const start = blankSetUp(host);
  const endpoint = textField({ label: 'Address devices reach this server at', hint: 'A name or an address. It starts as the name of this console.', value: start.endpoint });
  const port = textField({ label: 'UDP port', value: start.port });
  const address = textField({ label: 'This server\'s address and network on wg0', hint: 'The devices get the next addresses of this network.', value: start.address, mono: true });
  port.input.setAttribute('inputmode', 'numeric');
  const router = h('p', { class: 'small muted' }, routerWords(DEFAULTS.port));
  port.input.addEventListener('input', () => {
    router.textContent = routerWords(Number(port.input.value) || DEFAULTS.port);
  });
  const form = () => ({ endpoint: endpoint.input.value, port: port.input.value, address: address.input.value });
  return {
    elements: [endpoint.element, port.element, router, address.element],
    check() {
      const problems = setUpProblems(form());
      return { args: Object.keys(problems).length > 0 ? null : serveArgs(form()), problems };
    },
    fault(problems) {
      const fields = { endpoint, port, address };
      for (const name of Object.keys(fields).reverse()) if (problems[name]) fields[name].fault(problems[name]);
    },
  };
}

/**
 * Sets up private access with `args` (see `serveArgs`): makes wg0, then opens
 * WireGuard's UDP port in the firewall. True when both are made; a second
 * run with the same values changes nothing, so a failed one may be run again.
 * `where` is `{button, slot}` for the runs.
 */
export async function serveAndOpen(view, args, where) {
  const served = await view.api.run('private.serve', args, {
    ...where, words: { unlock: 'set up private access', running: 'Setting up private access', done: 'Private access is set up.', unchanged: 'Private access is set up already.' },
  });
  if (!served.ok) return false;
  const opened = await view.api.run('firewall.add', openPort(args.port ?? DEFAULTS.port), {
    ...where, words: { unlock: 'open WireGuard\'s port', running: 'Opening WireGuard\'s port', done: 'WireGuard\'s port is open.', unchanged: 'WireGuard\'s port is open already.' },
  });
  return opened.ok;
}
