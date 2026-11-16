// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Alerts page says, as plain values: the active alerts and the page
// each is about, the recent changes and the rows of Where alerts go. It reads
// what alertd answers, `alerts list --json` (`{source, alert: {key, severity,
// title, detail}, since, changed}`, seconds), `alerts history --json` (`{at,
// source, state, alert}`) and `alerts status --json` (channels-model.js).
// `time` is lib/time.js's.

import { channelArg, failing, toldWords } from './told-model.js';

/** The recent changes the page lists. */
export const RECENT = 20;

// The page each kind of alert is about, by the start of its key. The service
// worker has the same table for the notice's click (sw.js); a test holds the
// two to the same answers.
const PAGES = [
  [/^(smart|smart-rising|fs-full|fs-filling|btrfs-[a-z-]+):/, () => ['/storage', 'Storage']],
  [/^(backup-failed|backup-overdue):/, () => ['/snapshots/backups', 'Backups']],
  [/^service:(.+)$/, (found) => [`/services/${encodeURIComponent(found[1])}`, `${found[1]} service`]],
  [/^certificate:/, () => ['/certificates', 'Certificates']],
  [/^(temp:|memory-test$)/, () => ['/hardware', 'Hardware']],
  [/^(update-[a-z-]+|start-failed)$/, () => ['/updates#happened', 'What happened']],
];

/** The page an alert with `key` is about: `{path, label}`, or null for one that has none (the Alerts page itself). */
export function pageFor(key) {
  for (const [pattern, page] of PAGES) {
    const found = pattern.exec(String(key));
    if (found) {
      const [path, label] = page(found);
      return { path, label };
    }
  }
  return null;
}

const SEVERITY = Object.freeze({ critical: 'Critical', warning: 'Warning' });
const word = (severity) => SEVERITY[severity] ?? String(severity ?? '');

/** The active alerts, critical first and then the oldest: `{id, severity, tone, title, since, source, detail, page}`. */
export function activeRows(lines, time) {
  return lines
    .filter((line) => line?.alert)
    .sort((a, b) => (b.alert.severity === 'critical') - (a.alert.severity === 'critical') || (a.since ?? 0) - (b.since ?? 0))
    .map((line) => ({
      id: `${line.source}:${line.alert.key}`,
      severity: word(line.alert.severity),
      tone: line.alert.severity === 'critical' ? 'bad' : 'warn',
      title: line.alert.title ?? '',
      since: Number.isFinite(line.since) ? `since ${time.when(line.since)}` : '',
      source: line.source ?? '',
      detail: line.alert.detail ?? '',
      page: pageFor(line.alert.key),
    }));
}

const STATES = Object.freeze({
  raised: 'Raised', cleared: 'Cleared', event: 'Told', test: 'Test',
});

/** The last `RECENT` changes of the history, newest first: `{when, state, severity, tone, source, title}`. */
export function recentRows(lines, time) {
  return lines
    .filter((line) => Number.isFinite(line?.at) && line.alert)
    .sort((a, b) => b.at - a.at)
    .slice(0, RECENT)
    .map((line) => ({
      when: time.when(line.at),
      state: STATES[line.state] ?? String(line.state ?? ''),
      severity: word(line.alert.severity),
      tone: line.alert.severity === 'critical' ? 'bad' : 'warn',
      source: line.source ?? '',
      title: line.alert.title ?? '',
    }));
}

/** What a channel did last, in words: "Last sent Nov 13, 10:02 AM" and "Last failed 2:00 AM: ntfy answered 403 Forbidden". */
export function lastWords(channel, time) {
  const sent = Number.isFinite(channel.last_ok) ? `Last sent ${time.when(channel.last_ok)}` : null;
  const tried = Array.isArray(channel.last_error) ? channel.last_error : null;
  const failed = tried && Number.isFinite(tried[0]) ? `Last failed ${time.when(tried[0])}${tried[1] ? `: ${tried[1]}` : ''}` : null;
  return { sent, failed, none: sent === null && failed === null };
}

/**
 * The rows of Where alerts go, in alertd's order: `{name, told, sent, failed,
 * none, bad, id, waiting, here, arg, kind, channel}`. `pushId` is the ID this
 * browser kept, so that its row says so; `arg` is the channel's name for
 * `alerts.channel-about`, null when it has none, and `channel` the channel as
 * alertd gave it.
 */
export function channelRows(channels, time, pushId) {
  return channels.map((channel) => ({
    name: channel.kind === 'push' && channel.name ? `push · ${channel.name}` : channel.channel,
    told: toldWords(channel),
    ...lastWords(channel, time),
    bad: failing(channel),
    id: typeof channel.error_id === 'string' ? channel.error_id : null,
    waiting: channel.waiting > 0 ? `${channel.waiting} waiting` : null,
    here: channel.kind === 'push' && pushId !== null && channel.id === pushId,
    arg: channelArg(channel),
    kind: channel.kind,
    channel,
  }));
}

/** Whether a failed read says that alertd is not running: its socket has nobody on it. */
export const alertdDown = (error) => /alertd is not running/i.test([error?.command?.stderr, error?.detail, error?.title].filter(Boolean).join(' '));

/** What the page says when alertd is not running. */
export const DOWN_WORDS = 'alertd is not running, so nothing is told: ';

/** The command that starts alertd, as a person types it. */
export const START_COMMAND = 'elevate service alertd start';

/** What the page says when no channel is set. */
export const NO_CHANNEL_STATE = 'Alerts reach this server\'s screens only.';

/** The line above the rows, always the first row: alertd tells the screen of every change. */
export const SCREENS_WORDS = 'On this server\'s screens and at the next sign-in';

/** The action that takes a channel away, `{action, args, words}`, `words` saying what goes with it. */
export function removeCall(channel) {
  if (channel.kind === 'email') {
    return { action: 'alerts.email-remove', args: {}, words: 'The addresses, the mail server and its password are taken out of alerts.toml.' };
  }
  if (channel.kind === 'push') {
    return { action: 'alerts.push-remove', args: { id: channel.id }, words: 'That browser gets no more alerts from this server.' };
  }
  return { action: 'alerts.webhook-remove', args: { number: channel.number }, words: 'Its address and token are taken out of alerts.toml.' };
}
