// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The firewall's rules as plain values (design 10): a rule read and written as
// firewall(8) reads and writes it, in words, which rule decides a connection
// (the first that fits, as `firewall test` decides and as the kernel does), the
// form that makes a rule, and whether the rules after a change still let the
// browser reach the console (design 10.4). The order of deciding is held to
// userspace/programs/systools/src/firewall/cases.txt, which firewall.rs's tests
// read too.

import {
  formatNetwork, networkOf, parseAddress, parseNetwork, within,
} from '../../lib/address-model.js';

/** The program admind runs as, the only one a rule that names a program fits for a connection to the console. */
export const ADMIND = '/system/commands/admind';

/** The lowest and highest port. */
export const PORT_LOWEST = 1;
export const PORT_HIGHEST = 65_535;

const PROTOCOL_WORDS = ['tcp', 'udp', 'ping', 'any'];

/** Whether `name` is one an interface may have: 1 to 15 letters, digits, `.`, `-` and `_`. */
export const isCardName = (name) => /^[A-Za-z0-9._-]{1,15}$/.test(name);

/** The first and last port of `word`, `22` or `8000-8100`, or null. */
export function parsePorts(word) {
  const dash = word.indexOf('-');
  const first = dash < 0 ? word : word.slice(0, dash);
  const last = dash < 0 ? word : word.slice(dash + 1);
  if (!/^\d+$/.test(first) || !/^\d+$/.test(last)) return null;
  const ports = [Number(first), Number(last)];
  return ports[0] >= PORT_LOWEST && ports[0] <= ports[1] && ports[1] <= PORT_HIGHEST ? ports : null;
}

/** The ports as a rule writes them: `22`, or `8000-8100`. */
export const portsText = ([first, last]) => (first === last ? String(first) : `${first}-${last}`);

/**
 * A rule's words, `allow|deny [tcp|udp|ping|any] [PORT[-PORT]] [from NETWORK]
 * [on INTERFACE] [program PATH]`, as `Rule::parse` reads them: `{rule}` or
 * `{why}`. A rule is `{action, protocol, ports: [first, last] | null, from:
 * {address, prefix} | null, on, program}`.
 */
export function parseRule(words) {
  const [first, ...after] = words;
  if (first !== 'allow' && first !== 'deny') {
    return { why: first === undefined ? 'a rule starts with allow or deny' : `a rule starts with allow or deny, not \`${first}\`` };
  }
  const rule = {
    action: first, protocol: null, ports: null, from: null, on: null, program: null,
  };
  let rest = after;
  while (rest.length > 0) {
    const [word, ...tail] = rest;
    rest = tail;
    if (PROTOCOL_WORDS.includes(word) && rule.protocol === null) {
      rule.protocol = word;
    } else if ((word === 'from' && rule.from === null) || (word === 'on' && rule.on === null)) {
      if (rest.length === 0) {
        return { why: `\`${word}\` needs ${word === 'from' ? 'a network, such as 192.168.1.0/24' : 'an interface\'s name'} after it` };
      }
      const [value, ...more] = rest;
      rest = more;
      if (word === 'from') {
        rule.from = parseNetwork(value);
        if (rule.from === null) return { why: `\`${value}\` is not an address or network` };
      } else {
        if (!isCardName(value)) return { why: `\`${value}\` is not an interface's name` };
        rule.on = value;
      }
    } else if (word === 'program') {
      if (rest.length === 0) return { why: '`program` needs a program\'s path after it' };
      const path = rest.join(' ');
      if (!path.startsWith('/')) return { why: `\`program\` takes an absolute path, not \`${path}\`` };
      rule.program = path;
      rest = [];
    } else if (rule.ports === null && /^\d/.test(word)) {
      rule.ports = parsePorts(word);
      if (rule.ports === null) return { why: `\`${word}\` is not a port, 1 to 65535, or a range of them` };
    } else {
      return { why: `\`${word}\` is not a word of a rule here` };
    }
  }
  rule.protocol ??= 'any';
  if (rule.protocol === 'ping' && rule.ports !== null) return { why: 'a ping has no port' };
  if (rule.protocol === 'ping' && rule.program !== null) return { why: 'a ping reaches no program' };
  return { rule };
}

/** The rule as the settings file writes it: `allow tcp 443 from 192.168.1.0/24 on net0`. */
export function ruleText(rule) {
  return [
    rule.action,
    rule.protocol === 'any' ? null : rule.protocol,
    rule.ports && portsText(rule.ports),
    rule.from && `from ${formatNetwork(rule.from)}`,
    rule.on && `on ${rule.on}`,
    rule.program && `program ${rule.program}`,
  ].filter(Boolean).join(' ');
}

/** A rule of `firewall list --json`'s `rules` as a rule. */
export function ruleOf(entry) {
  return {
    action: entry.action,
    protocol: entry.protocol,
    ports: Array.isArray(entry.ports) ? [entry.ports[0], entry.ports[1]] : null,
    from: typeof entry.from === 'string' ? parseNetwork(entry.from) : null,
    on: entry.on ?? null,
    program: entry.program ?? null,
  };
}

/** The rules of `firewall list --json`'s answer, in order. */
export const rulesOf = (listed) => (Array.isArray(listed?.rules) ? listed.rules : []).map(ruleOf);

/** Whether two rules are the same rule, as `firewall remove` and `replace` take them to be. */
export const sameRule = (a, b) => ruleText(a) === ruleText(b);

// ---- in words ----------------------------------------------------------------------

/** What a rule is about: "TCP port 443", "UDP ports 5000 to 5100", "TCP on every port", "pings", "everything". */
function subjectOf(rule) {
  if (rule.protocol === 'ping') return 'pings';
  if (rule.protocol === 'any' && rule.ports === null) return 'everything';
  const protocol = { tcp: 'TCP', udp: 'UDP', any: 'TCP and UDP' }[rule.protocol];
  if (rule.ports === null) return `${protocol} on every port`;
  const [first, last] = rule.ports;
  return `${protocol} ${first === last ? `port ${first}` : `ports ${first} to ${last}`}`;
}

/** A rule in words: "Allow TCP port 443 from 192.168.1.0/24". */
export function wordsOf(rule) {
  return [
    rule.action === 'allow' ? 'Allow' : 'Deny',
    subjectOf(rule),
    rule.from && `from ${formatNetwork(rule.from)}`,
    rule.on && `on ${rule.on}`,
    rule.program && `for the program ${rule.program}`,
  ].filter(Boolean).join(' ');
}

/** What the machine does when no rule decides (design 10.3), which the board's "Default: allow / deny" does not say. */
export const DEFAULT_WORDS = 'A ping is answered. A connection reaches a port only while a service listens on it with the right to be reached; anything else reaches this machine alone.';

// ---- which rule decides ------------------------------------------------------------

/**
 * How `rule` fits `packet` (`{protocol, port, from, on, program}`, `from` an
 * address, each of `from`, `on` and `program` undefined when it is not
 * known), as the kernel matches them: `yes`, `no`, or `maybe` when it depends
 * on what the packet does not say.
 */
function fitOf(rule, packet) {
  const protocol = rule.protocol === 'any' || rule.protocol === packet.protocol;
  const port = packet.protocol === 'ping'
    ? rule.ports === null
    : rule.ports === null || (packet.port >= rule.ports[0] && packet.port <= rule.ports[1]);
  const known = (wanted, got, same) => {
    if (wanted === null) return true;
    return got === undefined ? null : same(wanted, got);
  };
  const from = known(rule.from, packet.from, (network, address) => within(address, network));
  const on = known(rule.on, packet.on, (name, card) => name === card);
  // No program listens for a ping, so a rule for a program never has it.
  const program = rule.program !== null && packet.protocol === 'ping' ? false : known(rule.program, packet.program, (path, listening) => path === listening);
  if (!protocol || !port || [from, on, program].includes(false)) return 'no';
  return [from, on, program].includes(null) ? 'maybe' : 'yes';
}

/**
 * Which rule of `rules` decides `packet`: the first that fits it, as `{number,
 * action, passed}` (`number` from 1). `passed` holds the numbers of the rules
 * before it that might apply, since the packet does not say enough. `number`
 * and `action` are null when no rule decides.
 */
export function decides(rules, packet) {
  const passed = [];
  for (const [at, rule] of rules.entries()) {
    const fit = fitOf(rule, packet);
    if (fit === 'yes') return { number: at + 1, action: rule.action, passed };
    if (fit === 'maybe') passed.push(at + 1);
  }
  return { number: null, action: null, passed };
}

/** The new connection the browser makes to the console, from the session's `from` and `via`, or null when the session does not say. */
export function browserPacket(session) {
  const from = parseAddress(session?.from);
  const via = session?.via;
  if (from === null || !Number.isInteger(via?.port) || typeof via.card !== 'string') return null;
  return { protocol: 'tcp', port: via.port, from, on: via.card, program: ADMIND };
}

/**
 * The rules after `change`: `{kind: 'add', rule}` appends, `{kind: 'replace',
 * old, rule}` puts `rule` where the first rule equal to `old` is, and
 * `{kind: 'remove', old}` takes that rule out. A change of a rule that is not
 * there leaves the rules as they are.
 */
export function rulesAfter(rules, change) {
  if (change.kind === 'add') return [...rules, change.rule];
  const at = rules.findIndex((rule) => sameRule(rule, change.old));
  if (at < 0) return rules;
  if (change.kind === 'replace') return rules.map((rule, place) => (place === at ? change.rule : rule));
  return rules.filter((_, place) => place !== at);
}

/**
 * Whether the browser still reaches the console once `change` is made
 * (design 10.4): `{state: 'allowed'}` when a rule allows the connection, or
 * none decides, which a service's port is reached by; `{state: 'denied',
 * number, rule, words}` when a rule denies it, `number` being the rule's place
 * among the rules now, which is the table the person sees; `{state:
 * 'unknown'}` when the session does not say where the browser comes from.
 * `words` is the sentence of the warning.
 */
export function connectionAfter(rules, change, session) {
  const packet = browserPacket(session);
  if (packet === null) return { state: 'unknown' };
  const after = rulesAfter(rules, change);
  const found = decides(after, packet);
  if (found.action !== 'deny') return { state: 'allowed' };
  const place = found.number - 1;
  const mine = change.kind !== 'remove' && sameRule(after[place], change.rule);
  // After a remove, the rules behind the one taken out have moved up a place.
  const removed = change.kind === 'remove' ? rules.findIndex((rule) => sameRule(rule, change.old)) : -1;
  const number = removed >= 0 && place >= removed ? found.number + 1 : found.number;
  const reaches = `The console reaches you from ${session.from} through ${session.via.card} on port ${session.via.port}`;
  return {
    state: 'denied',
    number,
    rule: after[place],
    words: `This would cut off your own connection. ${reaches}, and ${mine ? 'this rule' : `rule ${number}`} would deny it.`,
  };
}

// ---- the form that makes a rule ----------------------------------------------------

/** The protocols of the form, as the action takes them. */
export const PROTOCOLS = Object.freeze([
  { id: 'tcp', label: 'TCP' },
  { id: 'udp', label: 'UDP' },
  { id: 'any', label: 'TCP and UDP' },
  { id: 'ping', label: 'Ping' },
]);

/** The ready-made services of the form: one port each (design 10.3). */
export const CHIPS = Object.freeze([
  { id: 'ssh', label: 'SSH', protocol: 'tcp', ports: '22' },
  { id: 'web', label: 'Web', protocol: 'tcp', ports: '443' },
  { id: 'web-plain', label: 'Web without TLS', protocol: 'tcp', ports: '80' },
  { id: 'files', label: 'File sharing', protocol: 'tcp', ports: '445' },
  { id: 'mail', label: 'Mail', protocol: 'tcp', ports: '587' },
]);

/** Where a rule's traffic is from, for the form's From choice. */
export const FROM_KINDS = Object.freeze(['anywhere', 'network', 'address']);

/** The form of a new rule: allow TCP, no port yet, from anywhere, on any card. */
export const blankForm = () => ({
  action: 'allow', protocol: 'tcp', ports: '', from: { kind: 'anywhere', text: '' }, on: '',
});

/** The form's values for `rule`, for Change; `here` is the network text of "This network", or null. */
export function formOfRule(rule, here) {
  const from = rule.from === null ? null : formatNetwork(rule.from);
  const kind = from === null ? 'anywhere' : from === here ? 'network' : 'address';
  return {
    action: rule.action,
    protocol: rule.protocol,
    ports: rule.ports === null ? '' : portsText(rule.ports),
    from: { kind, text: kind === 'address' ? from : '' },
    on: rule.on ?? '',
  };
}

/**
 * The rule the form makes, as `{rule}` or `{field, why}` for the first value
 * that cannot work (`field` is `ports`, `from` or `on`). `here` is the
 * network of the card the browser came through, as text, or null.
 */
export function ruleOfForm(form, here) {
  const ping = form.protocol === 'ping';
  const ports = ping ? '' : form.ports.trim();
  if (!ping && ports === '') {
    return { field: 'ports', why: 'Name a port or a range of ports, as 22 or 8000-8100.' };
  }
  if (ports !== '' && parsePorts(ports) === null) {
    return { field: 'ports', why: `\`${ports}\` is not a port, 1 to 65535, or a range of them` };
  }
  let from = null;
  if (form.from.kind === 'network') from = here ?? '';
  if (form.from.kind === 'address') from = form.from.text.trim();
  if (from === '') return { field: 'from', why: 'Write an address or a network, such as 192.168.1.0/24.' };
  if (from !== null && parseNetwork(from) === null) return { field: 'from', why: `\`${from}\` is not an address or network` };
  if (form.on !== '' && !isCardName(form.on)) return { field: 'on', why: `\`${form.on}\` is not an interface's name` };
  const words = [form.action, form.protocol, ports, from && 'from', from, form.on && 'on', form.on].filter(Boolean);
  return parseRule(words);
}

/** The arguments of a rule's fields for an action: `prefix` is `old_` or `new_` for a replace. */
export function argsOfRule(rule, prefix = '') {
  const args = { [`${prefix}action`]: rule.action, [`${prefix}protocol`]: rule.protocol };
  if (rule.ports !== null) args[`${prefix}ports`] = portsText(rule.ports);
  if (rule.from !== null) args[`${prefix}from`] = formatNetwork(rule.from);
  if (rule.on !== null) args[`${prefix}on`] = rule.on;
  return args;
}

/** The action and its arguments for `change` (see `rulesAfter`), with `seconds` of a trial or none. */
export function actionOf(change, seconds = null) {
  const tried = seconds === null ? {} : { try: seconds };
  if (change.kind === 'add') return { action: 'firewall.add', args: { ...argsOfRule(change.rule), ...tried } };
  if (change.kind === 'remove') return { action: 'firewall.remove', args: { ...argsOfRule(change.old), ...tried } };
  return { action: 'firewall.replace', args: { ...argsOfRule(change.old, 'old_'), ...argsOfRule(change.rule, 'new_'), ...tried } };
}

/** Whether the page can change or remove `rule`: the actions have no field for a program. */
export const canChange = (rule) => rule.program === null;

/** The network of the card the browser came through that holds its address, as text, from `network show`'s answer and the session; null when it is not found. */
export function hereOf(show, session) {
  const via = session?.via;
  const card = (Array.isArray(show?.cards) ? show.cards : []).find((one) => one.name === via?.card);
  const held = card?.addresses?.find((one) => one.address === via.address);
  const network = held ? parseNetwork(`${held.address}/${held.prefix}`) : null;
  return network === null ? null : formatNetwork(networkOf(network));
}
