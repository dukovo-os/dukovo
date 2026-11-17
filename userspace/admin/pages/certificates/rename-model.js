// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The rules a machine's name follows, the full name made of a name and a
// domain, the lines of the dialog that says what a new name changes for each
// address the browser may reach the console by, and the address that keeps
// working once it is changed. The page reads this file when Change is pressed.

/** The most bytes of a whole name, and of one word of it. */
export const NAME_MAX = 64;
export const LABEL_MAX = 63;

const WORD = /^[A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?$/;

/** What is wrong with the full name `name`, or ''. */
export function fullNameProblem(name) {
  if (name === '') return 'Give the machine a name.';
  if (name.length > NAME_MAX) return `A name is ${NAME_MAX} characters at most.`;
  const words = name.split('.');
  if (words.some((word) => word.length > LABEL_MAX)) return `A word of the name is ${LABEL_MAX} characters at most.`;
  if (!words.every((word) => WORD.test(word))) return 'Use letters, digits and - in words, none starting or ending with -, with a dot between words.';
  return '';
}

/** The full name of a name and a domain; a domain may be written with a dot at its end or start. */
export function fullNameOf(first, domain) {
  const rest = String(domain ?? '').trim().replace(/^\.+|\.+$/g, '');
  return rest === '' ? String(first ?? '').trim() : `${String(first ?? '').trim()}.${rest}`;
}

/** What is wrong with the name and domain typed, or ''. */
export function nameProblem(first, domain) {
  if (String(first ?? '').trim().includes('.')) return 'The name is one word: put the rest in Domain.';
  return fullNameProblem(fullNameOf(first, domain));
}

/** The name on the local network of the full name `name`: its first word, with `.local`. */
export const localOf = (name) => `${String(name).split('.')[0].toLowerCase()}.local`;

/** The console's address, as a browser writes it: the port is left out when it is the standard one. */
export const originOf = (host, port) => `https://${host.includes(':') && !host.startsWith('[') ? `[${host}]` : host}${port && String(port) !== '443' ? `:${port}` : ''}/`;

/** The address the browser keeps using and the address that follows the name: `{kind, next}`. */
export function reachOf(host, machine, name) {
  const here = String(host ?? '').toLowerCase().replace(/\.$/, '');
  if (here === machine.local.toLowerCase()) return { kind: 'local', next: localOf(name) };
  if (here === machine.name.toLowerCase()) return { kind: 'full', next: name.toLowerCase() };
  if (/^\d{1,3}(\.\d{1,3}){3}$/.test(here) || here.includes(':')) return { kind: 'number', next: null };
  return { kind: 'other', next: null };
}

/**
 * The lines of the dialog that says what the new name `name` changes.
 * `host` is the address the browser reaches the console by, `port` the
 * console's, `address` the first address `console.about` gives (the one that
 * keeps working), or null.
 */
export function renameLines(machine, name, { host, port, address }) {
  const newLocal = localOf(name);
  const lines = [
    newLocal === machine.local
      ? `The name on the local network stays ${machine.local}.`
      : `The name on the local network becomes ${newLocal} at once; ${machine.local} stops answering.`,
    'This server makes its own certificate anew for the new name. Browsers and phones that trusted the old one warn again; its new fingerprint is shown when it is made.',
    'Certificates from Let\'s Encrypt do not change: they name the domain names you gave them.',
  ];
  const reach = reachOf(host, machine, name);
  const still = address ? originOf(address, port) : null;
  if (reach.kind === 'local' || reach.kind === 'full') {
    const there = originOf(reach.next, port);
    lines.push(reach.next === String(host).toLowerCase()
      ? `You reach this console as ${host}. It keeps that name.`
      : `You reach this console as ${host}. After the change it is at ${there}; this page takes you there.${still ? ` If that does not answer, the console is still at ${still}.` : ''}`);
  } else if (reach.kind === 'number') {
    lines.push(`You reach this console as ${host}. That address keeps working.`);
  } else {
    lines.push(`You reach this console as ${host}. It may not answer after the change.${still ? ` The console is still at ${still}.` : ''}`);
  }
  return lines;
}

/**
 * The address the link after the change goes to, `{host, url}`: the new full
 * name for a browser that used the old one, the new local name otherwise.
 * Null for a browser that came by number, which keeps working as it is.
 */
export function followAddress(host, machine, name, port) {
  const reach = reachOf(host, machine, name);
  if (reach.kind === 'number') return null;
  const next = reach.next ?? localOf(name);
  return { host: next, url: originOf(next, port) };
}

/** The fingerprint a `hostnamectl set-hostname --apply` says the new certificate has, in `output` lines; '' when it says none. */
export function fingerprintOf(output) {
  for (const line of Array.isArray(output) ? output : []) {
    const found = /SHA256\s+((?:[0-9A-Fa-f]{2}:)*[0-9A-Fa-f]{2})\b/.exec(String(line));
    if (found) return found[1].toUpperCase();
  }
  return '';
}
