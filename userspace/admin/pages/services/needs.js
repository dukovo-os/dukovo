// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the services' parts (design 3.5) make of `GET /api/1/services`, which
// is Spark's `show` document: `{now_ms, services: [{name, description,
// state, since_ms, failures, last_end, last_line, why, ...}]}`. The times in
// it count from the start of the machine, not from 1970.

/** The services of `show` that gave up: their state is `failed`. */
export function failedOf(show) {
  return (Array.isArray(show?.services) ? show.services : []).filter((service) => service.state === 'failed');
}

/** `words` with its first letter in capitals. */
const capital = (words) => words.charAt(0).toUpperCase() + words.slice(1);

/** A sentence ends in a full stop, a question mark or an exclamation mark. */
const ended = (words) => (/[.!?]$/.test(words) ? words : `${words}.`);

/**
 * What to tell of a failed service, from what Spark knows: how often its
 * start failed in a row, how its last run ended, the last line it wrote
 * and why it is not running. Each is left out when Spark has nothing.
 */
export function detailOf(service) {
  const parts = [];
  if (service.failures > 0) parts.push(`It gave up after ${service.failures} ${service.failures === 1 ? 'start' : 'starts'} that failed in a row.`);
  if (service.last_end) parts.push(`Its last run ${ended(service.last_end)}`);
  if (service.last_line) parts.push(`It last said: ${ended(service.last_line)}`);
  if (service.why) parts.push(ended(capital(service.why)));
  return parts.length > 0 ? parts.join(' ') : 'It is not running.';
}

/**
 * The needs of the Overview (design 3.5) for the services of `show`: each
 * failed one, worst first as the list gives them. `now` is the time in
 * Unix seconds; the time a service failed is worked out from how long it
 * has been in its state. `covers` says that the health check SEC-150,
 * which names the same services, is not listed again beside these.
 */
export function needsOf(show, now) {
  return failedOf(show).map((service) => {
    const ago = Number.isFinite(show.now_ms) && Number.isFinite(service.since_ms) ? Math.max(0, (show.now_ms - service.since_ms) / 1000) : null;
    return {
      id: `service-failed-${service.name}`,
      severity: 'bad',
      title: `${service.name} failed`,
      detail: detailOf(service),
      href: `/services/${encodeURIComponent(service.name)}`,
      icon: 'services',
      at: ago === null ? null : Math.floor(now - ago),
      covers: ['SEC-150'],
    };
  });
}

/** The badge of the Services page: how many services failed, or null when none did. */
export function badgeOf(show) {
  const count = failedOf(show).length;
  return count === 0 ? null : { count, label: 'failed', kind: 'bad' };
}
