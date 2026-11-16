// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Whether an update's reminder is put off now, by the rule the machine's own
// readers hold: `update settings --json` lists each reminder with `stands`
// from the last check the machine kept, and `update check --json` says
// `later_until_ms` on an offer it made with the advisories it read. The
// Updates page and the Overview read it the same way.

/**
 * The reminder that puts `offer` off at `now` (ms), `{until, by, at}`, or
 * null. `later` is the settings' list, or null when they were not read. An
 * entry puts off the version it was made for, until its time, while the
 * machine says it stands; a check just made may say so first.
 */
export function putOff(offer, later, now) {
  if (offer?.state !== 'update') return null;
  const entry = Array.isArray(later) ? later.find((one) => one?.item === offer.item) ?? null : null;
  const fresh = Number.isFinite(offer.later_until_ms) && offer.later_until_ms > now;
  if (entry) {
    const stands = entry.version === offer.to && ((entry.stands === true && entry.until_ms > now) || fresh);
    return stands ? { until: entry.until_ms, by: Number.isFinite(entry.by) ? entry.by : null, at: entry.at_ms } : null;
  }
  // Settings that were read and hold no entry say it is not put off, whatever an older check said.
  return !Array.isArray(later) && fresh ? { until: offer.later_until_ms, by: null, at: null } : null;
}

/** The reminders `update settings --json` lists, for the readers of the Overview; null when they could not be read. */
export async function readLater(cx) {
  const settings = await cx.cache('/api/1/updates/settings', 5000, () => cx.api.get('/api/1/updates/settings').catch(() => null));
  return Array.isArray(settings?.later) ? settings.later : null;
}
