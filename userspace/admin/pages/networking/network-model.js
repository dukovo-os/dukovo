// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Networking page says and decides (design 9), as plain values: the
// words of each card from `network show --json`, the connections from `ss -t
// --json`, the editor's form and its checks, and whether a change cuts off the
// browser. The checks are network/set.rs's `check`, sentence for sentence;
// userspace/programs/systools/src/network/cases.txt holds both to the same
// words.

import {
  addressOfHost, formatAddress, parseAddress, sameAddress, within,
} from '../../lib/address-model.js';

/** The most addresses and name servers a card is given. */
export const ADDRESSES_MAX = 4;
export const DNS_MAX = 3;

// ---- the editor's checks ---------------------------------------------------------

const CARD_WORDS = 'a card is named by 1 to 15 letters, digits, ., - and _, or by its MAC address';

/** Whether `card` is a MAC address: six groups of one or two hex digits between colons. */
const isMac = (card) => /^(?:[0-9a-fA-F]{1,2}:){5}[0-9a-fA-F]{1,2}$/.test(card);

/** Whether `card` may name a card: 1 to 15 letters, digits, `.`, `-` and `_` that start with a letter or a digit, or a MAC address. */
export const isCard = (card) => isMac(card) || /^[A-Za-z0-9][A-Za-z0-9._-]{0,14}$/.test(card);

/** Whether a host may have the IPv4 `bytes`: not in 0.0.0.0/8, loopback, multicast or the reserved 240.0.0.0/4. */
const unicast4 = (bytes) => bytes[0] !== 0 && bytes[0] !== 127 && bytes[0] < 224;

/** Whether a host may have the IPv6 `bytes`: not unspecified, loopback, multicast or IPv4-mapped. */
function unicast6(bytes) {
  const zeros = (count) => bytes.slice(0, count).every((byte) => byte === 0);
  const mapped = zeros(10) && bytes[10] === 0xff && bytes[11] === 0xff;
  return !(zeros(16) || (zeros(15) && bytes[15] === 1) || bytes[0] === 0xff || mapped);
}

/** Whether `address` is one a host may have. */
const unicast = (address) => (address.version === 4 ? unicast4(address.bytes) : unicast6(address.bytes));

/** Whether `address` only reaches its own link: 169.254.0.0/16 or fe80::/10. */
const linkLocal = (address) => (address.version === 4
  ? address.bytes[0] === 169 && address.bytes[1] === 254
  : address.bytes[0] === 0xfe && (address.bytes[1] & 0xc0) === 0x80);

/** The number an IPv4 address's four bytes make. */
const wordOfV4 = (bytes) => bytes.reduce((sum, byte) => sum * 256 + byte, 0);

/** An address of the card's own, `ADDRESS/PREFIX`: `{address, prefix}`, or `{field, why}`. */
function checkAddress(text) {
  const bad = (why) => ({ field: 'addresses', why });
  const slash = text.indexOf('/');
  if (slash < 0) return bad(`${text} has no prefix: write it as ADDRESS/PREFIX, as 192.168.1.10/24`);
  const word = text.slice(0, slash);
  const prefixWord = text.slice(slash + 1);
  const address = parseAddress(word);
  if (address === null) return bad(`${word} is not an IP address`);
  const prefix = /^\d+$/.test(prefixWord) ? Number(prefixWord) : Number.NaN;
  const most = address.version === 4 ? 32 : 128;
  if (!(prefix >= 1 && prefix <= most)) return bad(`${text}: an IPv${address.version} prefix is 1 to ${most}`);
  if (!unicast(address) || linkLocal(address)) return bad(`${formatAddress(address)} is not an address a card may have`);
  if (address.version === 4 && prefix < 31) {
    const word32 = wordOfV4(address.bytes);
    const size = 2 ** (32 - prefix);
    const network = word32 - (word32 % size);
    if (word32 === network) return bad(`${text} is the network's own address, not a card's`);
    if (word32 === network + size - 1) return bad(`${text} is the network's broadcast address, not a card's`);
  }
  return { address, prefix };
}

/** The words of an address, as typed, kept as a list of the non-empty ones. */
export const wordsOf = (text) => String(text ?? '').split(/[\s,]+/).filter(Boolean);

/**
 * The checks of `network set` that need no machine, in the command's order:
 * the card, metered, the mode, the addresses, the router, the name servers.
 * `set` is `{card, mode: 'dhcp' | 'fixed', addresses: [text], router: text |
 * null, dns: [text], metered: 'auto' | 'yes' | 'no' | null}`. Gives null when
 * the values can work, else `{field, why}` for the first that cannot: `field`
 * is the action's argument, and `why` a sentence; for an address, `at` is its
 * place in `addresses`.
 */
export function checkSet(set) {
  if (!isCard(set.card)) return { field: 'card', why: CARD_WORDS };
  if (set.metered != null && !['auto', 'yes', 'no'].includes(set.metered)) {
    return { field: 'metered', why: `metered is auto, yes or no, not ${set.metered}` };
  }
  const router = set.router || null;
  if (set.mode === 'dhcp') {
    if (set.addresses.length > 0 || router !== null || set.dns.length > 0) {
      return { field: 'addresses', why: 'a card that asks DHCP is given no addresses, router or DNS servers' };
    }
    return null;
  }
  if (set.addresses.length < 1 || set.addresses.length > ADDRESSES_MAX) {
    return { field: 'addresses', why: `a card has 1 to ${ADDRESSES_MAX} addresses of its own` };
  }
  const given = [];
  for (const [at, text] of set.addresses.entries()) {
    const found = checkAddress(text);
    if (found.field) return { ...found, at };
    if (given.some((one) => sameAddress(one.address, found.address))) {
      return { field: 'addresses', why: `${formatAddress(found.address)} is given twice`, at };
    }
    given.push(found);
  }
  if (router !== null) {
    const found = checkRouter(router, given);
    if (found) return found;
  }
  return checkDns(set.dns);
}

/** The router's check: an address a router may have, not the card's own, and on one of the card's networks. */
function checkRouter(text, given) {
  const address = parseAddress(text);
  if (address === null || !unicast(address)) return { field: 'router', why: `${text} is not an address a router may have` };
  if (given.some((one) => sameAddress(one.address, address))) {
    return { field: 'router', why: `${formatAddress(address)} is the card's own address, not its router` };
  }
  const same = given.filter((one) => one.address.version === address.version);
  // A card with an IPv6 address of its own reaches a router by its link-local address too.
  const onNetwork = address.version === 4
    ? same.some((one) => within(address, one))
    : same.length > 0 && (linkLocal(address) || same.some((one) => within(address, one)));
  if (onNetwork) return null;
  return { field: 'router', why: `the router ${formatAddress(address)} is on none of the networks of the card's addresses` };
}

/** The name servers' check: at most three, each an address a server may have, none twice. */
function checkDns(servers) {
  if (servers.length > DNS_MAX) return { field: 'dns', why: `a card has at most ${DNS_MAX} DNS servers` };
  const seen = [];
  for (const text of servers) {
    const address = parseAddress(text);
    if (address === null || !unicast(address)) return { field: 'dns', why: `${text} is not an address a DNS server may have` };
    if (seen.some((one) => sameAddress(one, address))) return { field: 'dns', why: `${formatAddress(address)} is given twice` };
    seen.push(address);
  }
  return null;
}

/** The hint under an address field: the mask of an IPv4 prefix, "/24 is 255.255.255.0", or how to write one. */
export function prefixHint(text) {
  const found = /\/(\d{1,2})$/.exec(String(text ?? '').trim());
  const address = parseAddress(String(text ?? '').split('/')[0].trim());
  const prefix = found ? Number(found[1]) : null;
  if (address?.version !== 4 || prefix === null || prefix < 1 || prefix > 32) {
    return 'Write each address with its prefix, as 192.168.1.10/24.';
  }
  const mask = [0, 1, 2, 3].map((at) => 256 - 2 ** (8 - Math.min(8, Math.max(0, prefix - at * 8)))).join('.');
  return `/${prefix} is ${mask}`;
}

// ---- the form ------------------------------------------------------------------

/** The choices of Metered, as the action takes them. */
export const METERED = Object.freeze([
  { id: 'auto', label: 'Decide by itself' },
  { id: 'yes', label: 'Yes' },
  { id: 'no', label: 'No' },
]);

/**
 * The editor's starting values for `card` (an entry of `network show`): what
 * its settings say, and for a card that asks DHCP, what it has now, so that a
 * person who chooses Fixed starts from the address in use, which a card that
 * changes from DHCP to a fixed address of the same value keeps its connections
 * on. `{mode, addresses, router, dns, metered}`.
 */
export function formOf(card) {
  const settings = card.settings ?? {};
  const own = Array.isArray(settings.addresses) ? settings.addresses : [];
  const inUse = (card.addresses ?? []).filter((held) => held.from !== 'link-local').map((held) => `${held.address}/${held.prefix}`);
  const dns = Array.isArray(settings.dns) && settings.dns.length > 0 ? settings.dns : (card.dns ?? []);
  return {
    mode: settings.mode === 'fixed' ? 'fixed' : 'dhcp',
    addresses: (own.length > 0 ? own : inUse).slice(0, ADDRESSES_MAX),
    router: settings.router ?? card.routers?.[0] ?? '',
    dns: dns.slice(0, DNS_MAX),
    metered: ['yes', 'no'].includes(settings.metered) ? settings.metered : 'auto',
  };
}

/** The values of `form` as `checkSet` takes them: blank fields left out, and for DHCP no addresses, router or name servers. */
export function setOf(form, card) {
  const fixed = form.mode === 'fixed';
  return {
    card: card.name,
    mode: fixed ? 'fixed' : 'dhcp',
    addresses: fixed ? form.addresses.map((text) => text.trim()).filter(Boolean) : [],
    router: fixed ? (wordsOf(form.router)[0] ?? null) : null,
    dns: fixed ? form.dns.flatMap(wordsOf) : [],
    metered: form.metered,
  };
}

/**
 * The arguments of `network.set` for `form`, with `seconds` of a trial or
 * none. `metered` goes only when it is not what the card's settings say, so
 * that a change of the address does not write a line the person never chose.
 */
export function argsOf(form, card, seconds = null) {
  const set = setOf(form, card);
  const args = { card: set.card, mode: set.mode };
  if (set.mode === 'fixed') {
    args.addresses = set.addresses.join(' ');
    if (set.router !== null) args.router = set.router;
    if (set.dns.length > 0) args.dns = set.dns.join(' ');
  }
  if (set.metered !== (card.settings?.metered ?? 'auto')) args.metered = set.metered;
  if (seconds !== null) args.try = seconds;
  return args;
}

// ---- does the change cut the browser off -----------------------------------------

/** The address the browser reaches the console by and the card that holds it, from the session's `via`; null when the session does not say. */
const viaOf = (session) => {
  const via = session?.via;
  const address = parseAddress(via?.address);
  return address === null || typeof via.card !== 'string' ? null : { address, card: via.card, port: via.port };
};

/** Whether `name` is `card`'s, by its name or its MAC address. */
const isCardNamed = (card, name) => card.name === name || card.mac?.toLowerCase() === String(name).toLowerCase();

/**
 * Whether the change `form` makes to `card` takes away what the browser
 * reaches the console by (design 9.4 and 8.5): the address it came to, or
 * the router of the card, which carries the replies when the browser is not on
 * the card's own network. `session` is `{from, via: {address, port, card}}`.
 * Gives null, or `{kind: 'address', address}` or `{kind: 'router', router}`;
 * a change to DHCP is taken to change the address, since DHCP chooses it.
 */
export function cutsOff(form, card, session) {
  const via = viaOf(session);
  if (via === null || !isCardNamed(card, via.card)) return null;
  const now = setOf(form, card);
  if (now.mode === 'dhcp') return card.settings?.mode === 'dhcp' ? null : { kind: 'address', address: formatAddress(via.address) };
  const addresses = now.addresses.map(parseNetworkOf).filter(Boolean);
  const own = addresses.find((one) => sameAddress(one.address, via.address));
  if (!own) return { kind: 'address', address: formatAddress(via.address) };
  const from = parseAddress(session.from);
  // A browser on the card's own network is reached without the router.
  if (from !== null && within(from, own)) return null;
  const router = card.settings?.router ?? card.routers?.[0] ?? null;
  if (router !== null && router !== (now.router ?? null)) return { kind: 'router', router };
  return null;
}

/** `ADDRESS/PREFIX` as `{address, prefix}`, or null. */
function parseNetworkOf(text) {
  const [word, prefix] = text.split('/');
  const address = parseAddress(word);
  return address === null || !/^\d+$/.test(prefix ?? '') ? null : { address, prefix: Number(prefix) };
}

/**
 * The address the console is reached at after the change, as a link: the
 * first address of the same kind as the one it was reached by, with the
 * console's port; null when the change gives none (DHCP chooses it).
 */
export function newAddressUrl(form, card, session, location) {
  const via = viaOf(session);
  const now = setOf(form, card);
  if (via === null || now.mode !== 'fixed') return null;
  const found = now.addresses.map(parseNetworkOf).find((one) => one && one.address.version === via.address.version);
  if (!found) return null;
  const host = found.address.version === 6 ? `[${formatAddress(found.address)}]` : formatAddress(found.address);
  const port = location.port ? `:${location.port}` : '';
  return `${location.protocol}//${host}${port}/networking`;
}

/** What the page says before Try it, for a change that cuts the browser off, as a list of paragraphs (design 8.5). */
export function cutWords(cut, { host, url }) {
  if (cut.kind === 'router') {
    return [
      `This change changes the router of this card, ${cut.router}, which carries the console's answers to this browser.`,
      'If the new router does not reach this browser, this page cannot reach the console. The change is undone by itself unless it is kept.',
    ];
  }
  if (addressOfHost(host) === null) {
    return [
      `This browser reaches the console as ${host}, which has ${cut.address} now, and this change takes that address away.`,
      'The name finds the new address within a minute, so this page reaches the console again over it. Choose a time that is long enough.',
    ];
  }
  const where = url === null
    ? 'the address DHCP gives the card'
    : url;
  return [
    `This browser reaches the console at ${cut.address}, which this change ${url === null ? 'may take' : 'takes'} away.`,
    `After Try it, open ${where} in a new tab, sign in, and keep the change there. The browser asks about the certificate again, since the machine's certificate names the machine, not its addresses.`,
  ];
}

// ---- cards in words --------------------------------------------------------------

const FROM_WORDS = Object.freeze({
  dhcp: 'DHCP', fixed: 'fixed', router: 'IPv6, from the router', 'link-local': 'link-local', other: 'set another way',
});

/** The speed of a link: "1 Gbit/s", "100 Mbit/s", "2.5 Gbit/s"; empty when the card says none. */
export function speedWords(mbps) {
  if (!Number.isFinite(mbps) || mbps <= 0) return '';
  return mbps >= 1000 ? `${mbps / 1000} Gbit/s` : `${mbps} Mbit/s`;
}

/** How a card's link is: `{kind, word}`, "Connected, 1 Gbit/s", or "No cable". */
export function stateOf(card) {
  if (!card.up) return { kind: 'warn', word: 'No cable' };
  const speed = speedWords(card.speed_mbps);
  return { kind: 'ok', word: speed ? `Connected, ${speed}` : 'Connected' };
}

/** How a card is set up, from `settings.mode`. */
export function modeWords(settings) {
  if (settings?.mode === 'fixed') return 'Has addresses of its own';
  if (settings?.mode === 'none') return 'Left as the machine finds it';
  return 'Gets its address by DHCP';
}

/** The words of `settings.metered`. */
export const meteredWords = (metered) => ({ yes: 'Metered', no: 'Not metered' })[metered] ?? 'Decided by itself';

/** The lease a card holds, in words, or null; `nowMs` is the time now and `time` lib/time.js's. */
export function leaseWords(lease, nowMs, time) {
  if (!lease) return null;
  if (lease.ends_ms == null) return `${lease.address} for ever`;
  if (lease.ends_ms <= nowMs) return `${lease.address}, which has ended`;
  return `${lease.address} until ${time.when(Math.floor(lease.ends_ms / 1000))}`;
}

/** An address and its prefix as a card shows it, with where it came from. */
export function heldOf(held) {
  return { text: `${held.address}/${held.prefix}`, from: FROM_WORDS[held.from] ?? FROM_WORDS.other, kind: held.from };
}

/**
 * The card in words, for the list: `{name, mac, state, mode, addresses,
 * routers, dns, metered, lease, through}`. `session` says which card the
 * browser came through.
 */
export function cardRow(card, session, nowMs, time) {
  return {
    name: card.name,
    mac: card.mac,
    state: stateOf(card),
    mode: modeWords(card.settings),
    addresses: (card.addresses ?? []).map(heldOf),
    routers: card.routers ?? [],
    dns: card.dns ?? [],
    metered: meteredWords(card.settings?.metered),
    lease: leaseWords(card.lease, nowMs, time),
    serves: card.settings?.dhcp_server === true,
    through: session?.via?.card === card.name,
    href: `/networking/${encodeURIComponent(card.name)}`,
  };
}

/** The card called `name` among `show`'s, by its name or its MAC address, or null. */
export const findCard = (show, name) => (Array.isArray(show?.cards) ? show.cards : []).find((card) => isCardNamed(card, name)) ?? null;

// ---- traffic and connections -----------------------------------------------------

/** The chart of one card's traffic, as lib/chart-model.js's `modelOf` takes it: bits a second, in and out. */
export const trafficChart = (name) => ({
  id: 'network',
  name: `${name} traffic`,
  now: `${name} now`,
  kind: 'bits',
  scale: 'peak',
  lines: [{ series: `net.rx{${name}}`, label: 'In' }, { series: `net.tx{${name}}`, label: 'Out' }],
});

/** One end of a connection as `ss` writes it: `192.168.1.10:9090`, or `[fe80::1]:22` for IPv6. */
export function endWords({ address, port }) {
  return `${address.includes(':') ? `[${address}]` : address}:${port === 0 ? '*' : port}`;
}

/** `established` as "Established", `close-wait` as "Close wait". */
const stateWords = (state) => (state ? state.charAt(0).toUpperCase() + state.slice(1).replaceAll('-', ' ') : 'Unknown');

/**
 * The rows of Connections from `ss -t --json`: `{local, remote, state,
 * thisBrowser}`, by local port and then by the other end. A row is this
 * browser's when it is the console's port and the browser's address.
 */
export function connectionRows(answer, session) {
  const rows = (Array.isArray(answer) ? answer : []).filter((row) => row?.tcp !== false && row?.local && row?.remote);
  const order = (a, b) => a.local.port - b.local.port
    || (a.remote.address < b.remote.address ? -1 : Number(a.remote.address > b.remote.address))
    || a.remote.port - b.remote.port;
  return rows.sort(order).map((row) => ({
    local: endWords(row.local),
    remote: endWords(row.remote),
    state: stateWords(row.state),
    thisBrowser: session?.via?.port === row.local.port && session?.from === row.remote.address,
  }));
}
