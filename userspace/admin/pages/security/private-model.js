// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What private access says (design 8.8 and 9.3) as plain values: whether it is
// set up and each device's row. They are made from `wireguard show --json`
// (design 11.10). Nothing here touches the page.

import { bytesWords } from '../../lib/size-words.js';

/** What private access is, for a server that has none. */
export const WHAT = 'A phone or laptop with the WireGuard app joins this server\'s own small network, wg0, from anywhere. The console stays off the internet and answers only on this network and through WireGuard.';

/** The interface private access is on. */
export const INTERFACE = 'wg0';

/** The interface of `show` (`wireguard show --json`), or null while private access is not set up. */
export const interfaceOf = (show) => (Array.isArray(show?.interfaces) ? show.interfaces.find((one) => one.name === INTERFACE) ?? null : null);

// ---- the devices ------------------------------------------------------------------------------

/** When a device last connected, in words: "2 minutes ago", "never"; `time` is lib/time.js's. */
export function handshakeWords(ms, time) {
  if (!Number.isFinite(ms) || ms <= 0) return 'Never connected';
  return `Connected ${time.relative(Math.floor(ms / 1000))}`;
}

/** What a device sent and received, from this server's side: "received 1.2 KB, sent 5.5 KB"; empty text when the counts are not there. */
export function transferWords(device) {
  if (!Number.isFinite(device?.received) || !Number.isFinite(device?.sent)) return '';
  return `received ${bytesWords(device.received)}, sent ${bytesWords(device.sent)}`;
}

/** The rows of the devices of `iface`: `{name, addresses, connected, transfer}`. */
export function deviceRows(iface, time) {
  return (Array.isArray(iface?.devices) ? iface.devices : []).map((device) => ({
    name: String(device.name),
    addresses: (Array.isArray(device.addresses) ? device.addresses : []).join(', '),
    connected: handshakeWords(device.latest_handshake_ms, time),
    transfer: transferWords(device),
  }));
}

/** The line that says what private access is: "wg0 · 10.8.0.1/24 · UDP port 51820 · devices reach it at home.example.org". */
export function interfaceWords(iface) {
  return [
    iface.name, iface.address, iface.port ? `UDP port ${iface.port}` : null,
    iface.endpoint ? `devices reach it at ${iface.endpoint}` : null, iface.up === false ? 'down' : null,
  ].filter(Boolean).join(' · ');
}
