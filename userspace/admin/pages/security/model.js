// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Security page decides and says, as plain values (design 8): the
// button each finding of the checks has, what a change of SSH's settings
// takes away, whether the browser would be cut off by a change of the
// console's reach, and the rows of the blocked addresses. Pure: nothing here
// touches the page.

import {
  formatNetwork, networkOf, parseAddress, parseNetwork, within,
} from '../../lib/address-model.js';
import { serviceHref } from '../services/needs.js';

// ---- the checks and their buttons (design 8.3) ---------------------------------------

/** The first sentence of `text`, with its full stop; the whole text when it has no second one. */
export function firstSentence(text) {
  const found = /^.*?[.!?](?=\s|$)/s.exec(String(text ?? '').trim());
  return found ? found[0] : String(text ?? '').trim();
}

/** The names that a check's detail lists before `ending`: "alice, bob and carol have no password" gives alice, bob and carol. */
function namesBefore(detail, ending) {
  const found = ending.exec(detail);
  if (!found) return [];
  return detail.slice(0, found.index).split(/,\s*|\s+and\s+/).map((name) => name.trim()).filter(Boolean);
}

const link = (label, href) => ({ kind: 'link', label, href });
const how = (words, command = null) => ({ kind: 'how', label: 'Show how', words, command });

/** The disk checks of the hardware check, which Storage shows to a console that has no Hardware page. */
const DISK_CHECKS = /^HW-11\d$/;

/** The warnings that name a disk or a filesystem, which Storage shows. */
const STORAGE_CHECKS = /^MET-10[1-4]$/;

/**
 * The buttons of one finding `{id, detail, fix}`: what each does is `kind`,
 * `link` (to `href`), `action` (runs `action` with `args`), `ssh` (opens
 * SSH's form with `preset`) or `how` (says `words`, and gives `command`
 * for a terminal). `has(id)` says whether the console has the page `id`.
 * A check this table does not know has a `how` button when it gave advice,
 * and none when it did not.
 */
export function fixesFor(finding, has) {
  const { id, detail = '', fix = '' } = finding;
  switch (id) {
    case 'SEC-101': return [link('See the rule', '/firewall')];
    case 'SEC-102': return [{
      kind: 'action', label: 'Start the firewall', action: 'service.start', args: { name: 'firewall' }, words: { unlock: 'start the firewall', done: 'The firewall is started. Check again to update the score.' },
    }];
    case 'SEC-110': return [{ kind: 'ssh', label: 'Keys only…', preset: 'keys-only' }];
    case 'SEC-111': return [{ kind: 'ssh', label: 'Choose who may sign in…', preset: 'who' }];
    case 'SEC-112': return [how('Remove the line on = false from /system/settings/blocking.toml. Blocking is on when the file says nothing of it.')];
    case 'SEC-120': {
      const names = namesBefore(detail, /\s+(?:has|have) no password\b/);
      return names.length === 0
        ? [link('Choose an account…', '/accounts')]
        : names.map((name) => link(names.length === 1 ? 'Set a password…' : `Set a password for ${name}…`, `/accounts/${encodeURIComponent(name)}`));
    }
    case 'SEC-121': return [link('Choose one…', '/accounts')];
    case 'SEC-130': return [has('updates') ? link('Updates', '/updates') : how(fix || 'Stage the update, and restart when it suits you.', 'elevate update apply')];
    case 'SEC-131': return [has('updates') ? link('Updates', '/updates') : how(fix || 'Ask for the update again.', 'elevate update check')];
    case 'SEC-140': return [how('The time service says what it tried.', 'timedatectl timesync-status')];
    case 'SEC-150': {
      const names = namesBefore(detail, /\s+gave up\b/);
      return names.length === 0 ? [link('See the services', '/services')] : names.map((name) => link(names.length === 1 ? 'See it' : `See ${name}`, serviceHref(name)));
    }
    case 'SEC-160': return [link('Snapshots', '/snapshots')];
    case 'SEC-170': return [link('Where alerts go', '/alerts')];
    default: break;
  }
  if (typeof id === 'string' && STORAGE_CHECKS.test(id)) return [link('See it', '/storage')];
  if (typeof id === 'string' && id.startsWith('MET-')) return [link('See it', '/')];
  if (typeof id === 'string' && id.startsWith('HW-')) {
    if (has('hardware')) return [link('See it', '/hardware')];
    return DISK_CHECKS.test(id) ? [link('See it', '/storage')] : (fix ? [how(fix)] : []);
  }
  return fix ? [how(fix)] : [];
}

// ---- who may sign in over SSH (design 8.6) ----------------------------------------------

/** Whether the names of `after` let in less than those of `before`: a list that names someone is narrower than none, and a list that leaves a name out is narrower than one that has it. */
function listNarrows(before, after) {
  return after.length > 0 && (before.length === 0 || before.some((name) => !after.includes(name)));
}

/** Whether `settings` let nobody sign in over SSH: neither passwords nor keys. The command refuses such a change. */
export const nobody = (settings) => !settings.passwords && !settings.keys;

/**
 * Whether going from the settings `before` to `after` (`{on, passwords,
 * keys, allow_users, allow_groups}`, as `ssh.show` gives them, a list that
 * is empty naming everyone) takes a way in from someone who has it now: SSH
 * off, a way of signing in off, or a list that leaves a name out. Such a
 * change is tried before it is kept, and is never applied at once. A change
 * that only adds ways in is not, and nor is any change of a file that lets
 * nobody in now, which has no one to cut off.
 */
export function narrows(before, after) {
  if (before.on === false || nobody(before)) return false;
  if (after.on === false) return true;
  return (before.passwords && !after.passwords)
    || (before.keys && !after.keys)
    || listNarrows(before.allow_users, after.allow_users)
    || listNarrows(before.allow_groups, after.allow_groups);
}

// ---- who may reach the console (design 8.7) ---------------------------------------------

/** The ranges the console answers when its reach is `private`, beside the networks of the machine's own cards (reach.rs): private, shared, loopback and link-local addresses. */
const PRIVATE_RANGES = Object.freeze(['10.0.0.0/8', '172.16.0.0/12', '192.168.0.0/16', '100.64.0.0/10', '169.254.0.0/16', '127.0.0.0/8', '::1/128', 'fc00::/7', 'fe80::/10'].map(parseNetwork));

/** `address` as an IPv4 address when it is one written as an IPv6 address, which the console answers as it would the IPv4 one. */
function plain(address) {
  const mapped = address.version === 6 && address.bytes.slice(0, 10).every((byte) => byte === 0) && address.bytes[10] === 0xff && address.bytes[11] === 0xff;
  return mapped ? { version: 4, bytes: address.bytes.slice(12) } : address;
}

/** Whether the network `inner` lies whole inside the network `outer`. */
const inside = (inner, outer) => inner.address.version === outer.address.version && inner.prefix >= outer.prefix && within(inner.address, outer);

/** Whether the address or network `text` is outside the private ranges; text that is neither is not said to be. */
export function isOutside(text) {
  const network = parseNetwork(text);
  if (network === null) return false;
  const own = { address: plain(network.address), prefix: network.prefix };
  return !PRIVATE_RANGES.some((range) => inside(own, range));
}

/** The networks of the machine's cards, as text, from `GET /api/1/network` (`cards[].addresses[{address, prefix}]`): what `reachCutsOff` takes. */
export function networksOf(show) {
  const found = (Array.isArray(show?.cards) ? show.cards : []).flatMap((card) => (Array.isArray(card.addresses) ? card.addresses : []));
  return found.map((one) => parseNetwork(`${one.address}/${one.prefix}`)).filter(Boolean).map((network) => formatNetwork(networkOf(network)));
}

/**
 * Whether making the console answer only the machines of this network and
 * the private ranges would cut off a browser that comes from `from`, as
 * `session.from` says it: its address is neither on one of the machine's
 * `networks` nor in a private range. An address that cannot be read is not
 * said to be cut off.
 */
export function reachCutsOff(from, networks) {
  const address = parseAddress(from);
  if (address === null) return false;
  const own = plain(address);
  return isOutside(from) && !networks.map(parseNetwork).some((network) => network !== null && within(own, network));
}

// ---- the addresses blocked after failed sign-ins (design 8.5) -------------------------------

/**
 * The rows of the blocked addresses, from the answer of `security.blocked`
 * (`blocks: [{address, until_ms, why}]`) and the person's `time` (lib/time.js):
 * `{key, address, outside, why, until, full}`. An address that is one host is
 * written without its prefix, as the commands write it, and `outside` says it
 * is not in a private range. `until` is when the block ends and `full` the
 * same to the second, for a title.
 */
export function blockedRows(answer, time) {
  const blocks = Array.isArray(answer?.blocks) ? answer.blocks : [];
  return blocks.map((block) => {
    const network = parseNetwork(String(block.address));
    const address = network === null ? String(block.address) : formatNetwork(network);
    const seconds = Math.floor(block.until_ms / 1000);
    return {
      key: String(block.address),
      address,
      outside: isOutside(address),
      why: block.why ?? '',
      until: time.when(seconds),
      full: time.full(seconds),
    };
  });
}
