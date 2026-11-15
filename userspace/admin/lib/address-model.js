// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// IP addresses and networks as plain values, read and written as the
// commands read and write them (Rust's std, which network(8) and firewall(8)
// use), so that the Networking and Firewall pages decide what the commands
// decide. An address is `{version: 4 | 6, bytes}`.

/** The four bytes of `text`, which is four decimal numbers without leading zeros, or null. */
function bytesOfV4(text) {
  const parts = text.split('.');
  if (parts.length !== 4) return null;
  const bytes = [];
  for (const part of parts) {
    if (!/^(?:0|[1-9]\d{0,2})$/.test(part)) return null;
    const value = Number(part);
    if (value > 255) return null;
    bytes.push(value);
  }
  return bytes;
}

/** The 16-bit groups of `part`, a run of groups between colons; the last may be an IPv4 address when `last` says so. Null when it is not one. */
function groupsOf(part, last) {
  if (part === '') return [];
  const pieces = part.split(':');
  const groups = [];
  for (const [at, piece] of pieces.entries()) {
    if (piece.includes('.')) {
      const bytes = last && at === pieces.length - 1 ? bytesOfV4(piece) : null;
      if (bytes === null) return null;
      groups.push(bytes[0] * 256 + bytes[1], bytes[2] * 256 + bytes[3]);
    } else if (/^[0-9a-fA-F]{1,4}$/.test(piece)) {
      groups.push(Number.parseInt(piece, 16));
    } else {
      return null;
    }
  }
  return groups;
}

/** The sixteen bytes of `text`, an IPv6 address with at most one `::` and perhaps an IPv4 address at its end, or null. */
function bytesOfV6(text) {
  if (!/^[0-9a-fA-F:.]+$/.test(text)) return null;
  const sides = text.split('::');
  if (sides.length > 2) return null;
  let groups;
  if (sides.length === 1) {
    groups = groupsOf(text, true);
    if (groups === null || groups.length !== 8) return null;
  } else {
    const head = groupsOf(sides[0], false);
    const tail = groupsOf(sides[1], true);
    // `::` stands for at least one group.
    if (head === null || tail === null || head.length + tail.length > 7) return null;
    groups = [...head, ...new Array(8 - head.length - tail.length).fill(0), ...tail];
  }
  return groups.flatMap((group) => [group >> 8, group & 0xff]);
}

/** The address `text` writes, or null when it is not an IPv4 or IPv6 address. */
export function parseAddress(text) {
  if (typeof text !== 'string') return null;
  const v4 = bytesOfV4(text);
  if (v4 !== null) return { version: 4, bytes: v4 };
  const v6 = bytesOfV6(text);
  return v6 === null ? null : { version: 6, bytes: v6 };
}

/** The groups of an IPv6 address's bytes. */
const groupsOfBytes = (bytes) => Array.from({ length: 8 }, (_, at) => bytes[at * 2] * 256 + bytes[at * 2 + 1]);

/** An IPv6 address as Rust writes it: lower case, the first longest run of two or more zero groups as `::`, and `::ffff:a.b.c.d` for an IPv4-mapped one. */
function textOfV6(bytes) {
  const groups = groupsOfBytes(bytes);
  if (groups.slice(0, 5).every((group) => group === 0) && groups[5] === 0xffff) {
    return `::ffff:${bytes.slice(12).join('.')}`;
  }
  let best = { at: -1, length: 0 };
  for (let at = 0; at < groups.length;) {
    if (groups[at] !== 0) {
      at += 1;
    } else {
      let end = at;
      while (end < groups.length && groups[end] === 0) end += 1;
      if (end - at > best.length) best = { at, length: end - at };
      at = end;
    }
  }
  const hex = (list) => list.map((group) => group.toString(16)).join(':');
  if (best.length < 2) return hex(groups);
  return `${hex(groups.slice(0, best.at))}::${hex(groups.slice(best.at + best.length))}`;
}

/** `address` as text, as the commands print it. */
export const formatAddress = (address) => (address.version === 4 ? address.bytes.join('.') : textOfV6(address.bytes));

/** The length of `address` in bits: 32 or 128. */
export const wholeLength = (address) => (address.version === 4 ? 32 : 128);

/**
 * The network of `text`, `ADDRESS/PREFIX` or an address alone, which is its
 * version's whole length: `{address, prefix}`, or null when it is not one.
 */
export function parseNetwork(text) {
  if (typeof text !== 'string') return null;
  const [word, prefixWord, ...rest] = text.split('/');
  const address = parseAddress(word);
  if (address === null || rest.length > 0) return null;
  if (prefixWord === undefined) return { address, prefix: wholeLength(address) };
  if (!/^\d{1,3}$/.test(prefixWord) || Number(prefixWord) > wholeLength(address)) return null;
  return { address, prefix: Number(prefixWord) };
}

/** A network as a rule writes it: the address alone when the prefix is the whole length. */
export function formatNetwork({ address, prefix }) {
  return prefix === wholeLength(address) ? formatAddress(address) : `${formatAddress(address)}/${prefix}`;
}

/** The network `{address, prefix}` holds: its first address, with the host bits cleared. */
export function networkOf({ address, prefix }) {
  const bytes = address.bytes.map((byte, at) => {
    const bits = Math.min(8, Math.max(0, prefix - at * 8));
    return byte & ((0xff << (8 - bits)) & 0xff);
  });
  return { address: { version: address.version, bytes }, prefix };
}

/** Whether `address` is in the network `{address, prefix}` of its own version: an IPv4 address is in no IPv6 network, and the reverse. */
export function within(address, network) {
  if (address.version !== network.address.version) return false;
  let bits = network.prefix;
  for (let at = 0; bits > 0; at += 1, bits -= 8) {
    const mask = bits >= 8 ? 0xff : (0xff << (8 - bits)) & 0xff;
    if ((address.bytes[at] & mask) !== (network.address.bytes[at] & mask)) return false;
  }
  return true;
}

/** Whether two addresses are the same. */
export const sameAddress = (a, b) => a.version === b.version && a.bytes.every((byte, at) => byte === b.bytes[at]);

/** The address `host`, as `location.hostname` gives it, is (an IPv6 one is in brackets), or null for a name. */
export function addressOfHost(host) {
  const bare = typeof host === 'string' && host.startsWith('[') && host.endsWith(']') ? host.slice(1, -1) : host;
  return parseAddress(bare);
}
