// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the forms of private access ask (design 8.8) as plain values: the set-up
// form's fields and what they send, what a device reaches and a device's name.
// Nothing here touches the page.

import { parseNetwork, wholeLength } from '../../lib/address-model.js';

/** What Set up offers when the person writes nothing: wireguard(8)'s own defaults. */
export const DEFAULTS = Object.freeze({ port: 51_820, address: '10.8.0.1/24' });

/**
 * What a device reaches: this server's address on wg0, and so the console and
 * the server's other services. Dukovo does not route between its network
 * cards, so not the other machines of the server's networks.
 */
export const REACH_WORDS = 'The device reaches this server through wg0: the console and the server\'s other services, not the other machines of its networks.';

/** A device's name: 1 to 32 letters, digits, `-` and `_`, not starting with `-` (`wireguard add`'s rule). */
export const NAME_RULE = /^[A-Za-z0-9_][A-Za-z0-9_-]{0,31}$/;

/** What is wrong with the device's name, as a sentence, or empty text; `taken` are the names of the devices there are. */
export function nameProblem(name, taken = []) {
  const word = String(name ?? '').trim();
  if (word === '') return 'Write a name for the device, such as phone.';
  if (!NAME_RULE.test(word)) return 'Use 1 to 32 letters, digits, - and _, not starting with -.';
  return taken.includes(word) ? 'A device already has that name.' : '';
}

/** The file a device's settings are saved as. */
export const fileName = (name) => `${name}.conf`;

// ---- the set-up form ------------------------------------------------------------------------

/** The form's fields before the person writes: the console's host name as the address devices reach, and the defaults. */
export const blankSetUp = (host) => ({ endpoint: host ?? '', port: String(DEFAULTS.port), address: DEFAULTS.address });

/** Whether `text` is a host name or an address a device can reach: 1 to 253 bytes, no spaces or slashes. */
const isEndpoint = (text) => text.length >= 1 && new TextEncoder().encode(text).length <= 253 && !/[\s/]/.test(text);

/** The problems of the form `{endpoint, port, address}` by field; empty when it is fit to send. */
export function setUpProblems(form) {
  const found = {};
  if (!isEndpoint(String(form.endpoint).trim())) found.endpoint = 'Write the name or address that devices reach this server at.';
  const port = Number(form.port);
  if (String(form.port).trim() === '' || !Number.isInteger(port) || port < 1024 || port > 65_535) found.port = 'Write a port from 1024 to 65535.';
  const network = parseNetwork(String(form.address).trim());
  const [low, high] = network?.address.version === 6 ? [48, 120] : [16, 30];
  if (network === null || !String(form.address).includes('/') || network.prefix < low || network.prefix > high || network.prefix === wholeLength(network.address)) {
    found.address = 'Write the interface\'s address and prefix, such as 10.8.0.1/24 (a prefix of 16 to 30, or 48 to 120 for IPv6).';
  }
  return found;
}

/** The arguments of `private.serve` for the form: the address and port only when they are not the defaults. */
export function serveArgs(form) {
  const args = { endpoint: String(form.endpoint).trim() };
  if (Number(form.port) !== DEFAULTS.port) args.port = Number(form.port);
  if (String(form.address).trim() !== DEFAULTS.address) args.address = String(form.address).trim();
  return args;
}

/** The firewall rule that opens WireGuard's port, as `firewall.add` takes it. */
export const openPort = (port) => ({ action: 'allow', protocol: 'udp', ports: String(port) });

/** What stands under the address field: the router sends the port here. */
export const routerWords = (port) => `Your router must send UDP port ${port} to this server.`;

/**
 * The device's file in the answer of `private.add`: a change answers
 * `{done, changed, output}`, and `wireguard add NAME --json` prints the device
 * as one line of `output`, with its file as `config`. Null when no line holds
 * one, so that the page never shows an empty file as if it were the device's.
 */
export function deviceFile(answer) {
  const lines = Array.isArray(answer?.output) ? answer.output : [];
  for (const line of lines) {
    let device;
    try {
      device = JSON.parse(line);
    } catch {
      continue;
    }
    if (typeof device?.config === 'string' && device.config.includes('[Interface]')) return device.config;
  }
  return null;
}
