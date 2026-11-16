// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the services' parts (design 3.5) make of `GET /api/1/services`, which
// is Spark's `show` document: `{now_ms, services: [{name, description,
// state, since_ms, failures, last_end, last_line, why, ...}], trials: [{id,
// left_ms, ...}]}`. The times in it count from the start of the machine, not
// from 1970.

/**
 * The address of the page of the service `name`. A dot is written out: the
 * server answers a path whose last part has a dot as a file, and a name of a
 * service may have one.
 */
export const serviceHref = (name) => `/services/${encodeURIComponent(name).replaceAll('.', '%2E')}`;

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
      href: serviceHref(service.name),
      icon: 'services',
      at: ago === null ? null : Math.floor(now - ago),
      covers: ['SEC-150'],
    };
  });
}

// The trials that have a page of their own (design 8.6), by Spark's name for
// them. `service.NAME` is a `service try`'s, whose page is the service's.
const TRIAL_PAGES = Object.freeze({
  network: { title: 'Network settings are being tried', href: '/networking', icon: 'network' },
  firewall: { title: 'Firewall rules are being tried', href: '/firewall', icon: 'firewall' },
  ssh: { title: 'SSH settings are being tried', href: '/security', icon: 'security' },
  console: { title: 'Settings of who may reach the console are being tried', href: '/security', icon: 'security' },
});

/** What a trial called `id` is, for the Overview: its words, the page that shows it and its icon. */
function trialPage(id) {
  if (Object.hasOwn(TRIAL_PAGES, id)) return TRIAL_PAGES[id];
  if (id.startsWith('service.') && id.length > 'service.'.length) {
    const name = id.slice('service.'.length);
    return { title: `The settings of ${name} are being tried`, href: serviceHref(name), icon: 'services' };
  }
  return { title: 'Settings are being tried', href: null, icon: 'info' };
}

/**
 * The needs of the Overview (design 8.6) for the trials of `show`: a change
 * that Spark undoes unless someone keeps it. Each is a warning that says
 * when it is undone, as the time of day and not a countdown, so that the
 * list stays still. `now` is the time in Unix seconds and `time` the
 * person's formatters (lib/time.js).
 */
export function trialNeeds(show, now, time) {
  const trials = Array.isArray(show?.trials) ? show.trials : [];
  return trials.filter((trial) => typeof trial?.id === 'string' && trial.id !== '').map((trial) => {
    const page = trialPage(trial.id);
    const ends = Number.isFinite(trial.left_ms) ? Math.floor(now + trial.left_ms / 1000) : null;
    return {
      id: `trial-${trial.id}`,
      severity: 'warn',
      title: page.title,
      detail: ends === null ? 'They are undone unless they are kept.' : `They are undone at ${time.when(ends)} unless they are kept.`,
      href: page.href,
      icon: page.icon,
      at: null,
      covers: [],
    };
  });
}

/** The badge of the Services page: how many services failed, or null when none did. */
export function badgeOf(show) {
  const count = failedOf(show).length;
  return count === 0 ? null : { count, label: 'failed', kind: 'bad' };
}
