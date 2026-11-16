// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What a channel is told of, in words, and the arguments that change it: the
// kinds of alert, whether a warning or only a critical alert goes, and
// whether a clearing does.

import {
  KIND_IDS, KINDS, joinWords, kindsOf,
} from './channels-model.js';

const lower = (label) => label.toLowerCase();

/** "every kind", or the kinds in words: "data at risk, hardware". */
export function kindsWords(channel) {
  const told = kindsOf(channel);
  if (told.length === KIND_IDS.length) return 'every kind';
  return told.map((id) => lower(KINDS.find((kind) => kind.id === id).label)).join(', ');
}

/** What a channel is told, as one line: "Told of: data at risk · critical only, and when one clears". */
export function toldWords(channel) {
  const alerts = channel?.severity === 'critical' ? 'critical only' : 'every alert';
  return `Told of: ${kindsWords(channel)} · ${alerts}${channel?.cleared === true ? ', and when one clears' : ''}`;
}

/** Whether the channel's last try failed: it has an error and no success after it. */
export const failing = (channel) => Array.isArray(channel?.last_error)
  && (!Number.isFinite(channel.last_ok) || channel.last_error[0] > channel.last_ok);

/** "Hardware alerts reach no channel: only this server's screens.", or null when every kind reaches a channel or none is set. */
export function uncoveredWords(channels) {
  if (channels.length === 0) return null;
  const left = KINDS.filter((kind) => !channels.some((channel) => kindsOf(channel).includes(kind.id))).map((kind) => kind.label);
  if (left.length === 0) return null;
  return `${joinWords([left[0], ...left.slice(1).map(lower)])} alerts reach no channel: only this server's screens.`;
}

/** The `about` argument for the kinds `ids`: `all` for every kind, else the words with commas. */
export function aboutArg(ids) {
  const chosen = KIND_IDS.filter((id) => ids.includes(id));
  return chosen.length === KIND_IDS.length ? 'all' : chosen.join(',');
}

/** The `channel` argument of `alerts.channel-about`: `email`, `webhook:N` or `push:ID`; null for a channel that has none. */
export function channelArg(channel) {
  if (channel.kind === 'email') return 'email';
  if (channel.kind === 'push') return typeof channel.id === 'string' ? `push:${channel.id}` : null;
  return Number.isInteger(channel.number) ? `webhook:${channel.number}` : null;
}
