// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the console says of alertd's channels, as plain values, for the Alerts
// page and the Alerts card of Security. The channels are `alerts status
// --json`, a line each: `{channel, kind, host, severity, cleared, about,
// number, id, name, last_ok, last_error: [seconds, words], error_id, waiting}`.
// It imports nothing, since Security reads it on every visit.

/** The kinds of alert a channel may be told of, as alerts.toml(5) names them in `about`. */
export const KINDS = Object.freeze([
  { id: 'data', label: 'Data at risk' },
  { id: 'stopped', label: 'Something stopped' },
  { id: 'hardware', label: 'Hardware' },
]);

export const KIND_IDS = KINDS.map((kind) => kind.id);

/** The lines that alertd answers a list with: the array itself. */
export const listOf = (answer) => (Array.isArray(answer) ? answer : []);

/** "a", "a and b", "a, b and c". */
export function joinWords(words) {
  if (words.length < 2) return words.join('');
  return `${words.slice(0, -1).join(', ')} and ${words[words.length - 1]}`;
}

/** The ids of the kinds that `channel` is told of: all of them unless its `about` names some. */
export function kindsOf(channel) {
  const named = Array.isArray(channel?.about) ? KIND_IDS.filter((id) => channel.about.includes(id)) : [];
  return named.length > 0 ? named : KIND_IDS;
}

/** The name a channel goes by in a sentence: "email" for the email channel, whatever address it holds, else alertd's own. */
export const shortName = (channel) => (channel.kind === 'email' ? 'email' : channel.channel);

/** "email and ntfy (ntfy.sh)", or null when no channel is set. */
export function channelsWords(channels) {
  return channels.length === 0 ? null : joinWords(channels.map(shortName));
}

/**
 * A row for each kind of alert: `{id, label, words, tone}`. `words` names the
 * channels told of the kind, "(critical only)" after one that tells no
 * warnings, or says that none is.
 */
export function kindRows(channels) {
  return KINDS.map((kind) => {
    const told = channels.filter((channel) => kindsOf(channel).includes(kind.id));
    const names = told.map((channel) => `${shortName(channel)}${channel.severity === 'critical' ? ' (critical only)' : ''}`);
    return {
      id: kind.id, label: kind.label, words: told.length === 0 ? 'no channel' : joinWords(names), tone: told.length === 0 ? 'warn' : 'ok',
    };
  });
}
