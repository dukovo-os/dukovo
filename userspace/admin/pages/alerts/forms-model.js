// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The email and webhook forms as plain values: what each field may hold, with
// the rules alertd applies to alerts.toml (so that a mistake is told at the
// field, not after the trip), and the arguments each action takes. A secret
// is never in what is built here for showing: the Show command panel gets an
// empty string in its place.

import { aboutArg } from './told-model.js';

/** The most addresses an email channel takes. */
export const MOST_ADDRESSES = 8;

/** The kinds of webhook, with the words of the form. `token` says whether the kind takes one: `no`, `maybe` or `must`. */
export const WEBHOOKS = Object.freeze([
  { id: 'ntfy', label: 'ntfy', token: 'maybe' },
  { id: 'matrix', label: 'Matrix', token: 'must' },
  { id: 'slack', label: 'Slack', token: 'no' },
  { id: 'discord', label: 'Discord', token: 'no' },
  { id: 'json', label: 'Any address that takes a POST', token: 'maybe' },
]);

const hostOk = (host) => host !== '' && host.length <= 253 && /^[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?$/.test(host);

/** Whether `address` is `local@domain` in the plain form that mail is sent to (alertd's rule). */
export function isAddress(address) {
  const at = address.indexOf('@');
  if (at < 1) return false;
  const local = address.slice(0, at);
  return local.length <= 64 && /^[!#$%&'*+\-/=?^_`{|}~.A-Za-z0-9]+$/.test(local) && hostOk(address.slice(at + 1));
}

/** Whether `host` is a host name or an IPv4 address (alertd's rule). */
export const isHost = hostOk;

/** The addresses in `text`, one a line or with commas, each once. */
export const addressesOf = (text) => [...new Set(text.split(/[\n,]/).map((one) => one.trim()).filter(Boolean))];

/**
 * What is wrong with the email form's `values` (`{to, server, user, from}`,
 * text), as `{field: words}`; `{}` when nothing is.
 */
export function emailProblems(values) {
  const found = {};
  const to = addressesOf(values.to);
  const bad = to.find((address) => !isAddress(address));
  if (to.length === 0) found.to = 'Type at least one address.';
  else if (to.length > MOST_ADDRESSES) found.to = `Type at most ${MOST_ADDRESSES} addresses.`;
  else if (bad) found.to = `${bad} is not an email address.`;
  const server = values.server.trim();
  if (server === '') found.server = 'Type the name of the mail server.';
  else if (!isHost(server)) found.server = `${server} is not a host name. The port goes in Port.`;
  if (values.from.trim() !== '' && !isAddress(values.from.trim())) found.from = `${values.from.trim()} is not an email address.`;
  return found;
}

/** The `about` argument, left out for every kind (the action's default). */
const aboutPart = (ids) => (aboutArg(ids) === 'all' ? {} : { about: aboutArg(ids) });

/** The arguments of `alerts.email`; `password` is left out when there is none, which keeps the one the channel has. */
export function emailArgs(values, password) {
  const user = values.user.trim();
  const from = values.from.trim();
  return {
    to: addressesOf(values.to).join('\n'),
    server: values.server.trim(),
    port: values.port,
    ...(user === '' ? {} : { user }),
    ...(from === '' ? {} : { from }),
    severity: values.severity,
    cleared: values.cleared,
    ...aboutPart(values.about),
    ...(password ? { password } : {}),
  };
}

const PRIVATE_V4 = /^(127\.|10\.|192\.168\.|169\.254\.|172\.(1[6-9]|2\d|3[01])\.)/;

/** Whether `host` is localhost or an address on the owner's own network, the only place `json` may use plain HTTP. */
export function isPrivateHost(host) {
  const bare = host.replace(/^\[|\]$/g, '').toLowerCase();
  return bare === 'localhost' || bare === '::1' || PRIVATE_V4.test(bare) || /^(f[cd][0-9a-f]{2}|fe[89ab][0-9a-f]):/.test(bare);
}

/** What is wrong with a webhook's address for `kind`, or null. */
export function urlProblem(kind, url) {
  let parsed;
  try {
    parsed = new URL(url);
  } catch {
    return 'Type the whole address, starting with https://.';
  }
  if (url.length > 2048) return 'The address is longer than 2,048 characters.';
  if (parsed.protocol === 'https:') return null;
  if (parsed.protocol !== 'http:') return 'The address starts with https://.';
  if (kind !== 'json') return 'The address starts with https://: this kind is sent over https only.';
  return isPrivateHost(parsed.hostname) ? null : 'A plain http address has to be on this network: use https, or a machine of this network.';
}

/** What is wrong with a webhook's token for `kind`, or null. */
export function tokenProblem(kind, token) {
  if (/[\s\u0000-\u001f\u007f]/.test(token)) return 'The token has a space or a line break in it.';
  if (token.length > 1024) return 'The token is longer than 1,024 characters.';
  if (kind === 'matrix' && token === '') return 'Matrix needs an access token.';
  return null;
}

/** What is wrong with a Matrix room's ID, or null. */
export const roomProblem = (kind, room) => (kind === 'matrix' && !/^![^:]+:.+$/.test(room.trim()) ? 'Type the room\'s ID, such as !abc:example.org.' : null);

/** Whether the form of `kind` has a token field. */
export const takesToken = (kind) => (WEBHOOKS.find((one) => one.id === kind)?.token ?? 'no') !== 'no';

/** The arguments of `alerts.webhook-add`; `url` and `token` are the secrets, `''` for the panel that shows the command. */
export function webhookArgs(values, url, token) {
  const room = values.room.trim();
  return {
    kind: values.kind,
    url,
    ...(takesToken(values.kind) && token !== '' ? { token } : {}),
    ...(values.kind === 'matrix' && room !== '' ? { room } : {}),
    severity: values.severity,
    cleared: values.cleared,
    ...aboutPart(values.about),
  };
}
