// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the palette finds of services (design 3.5): each service Spark runs,
// with its state to filter by (`state:failed`), and a row of its own for
// start, stop, restart, enable and disable ("Restart nginx"). Until the
// Services page is there, Enter on a service explains it.

// The words Spark says a service is in, the failed ones first.
export const STATES = ['failed', 'running', 'stopped', 'disabled', 'starting', 'pending', 'waiting', 'stopping', 'done'];

/** The service rows of an answer of `GET /api/1/services`: Spark's document, or its list of services. Failed services come first, then by name. */
export function itemsOf(answer) {
  const services = Array.isArray(answer) ? answer : answer?.services;
  return (Array.isArray(services) ? services : [])
    .filter((service) => typeof service?.name === 'string' && service.name !== '')
    .map((service) => ({
      key: service.name,
      title: service.name,
      detail: typeof service.description === 'string' ? service.description : '',
      state: typeof service.state === 'string' ? service.state : '',
      href: `/services/${encodeURIComponent(service.name)}`,
      action: 'service.explain',
      args: { name: service.name },
    }))
    .sort((a, b) => Number(b.state === 'failed') - Number(a.state === 'failed') || (a.title < b.title ? -1 : a.title > b.title ? 1 : 0));
}

export const providers = [{
  group: 'Services',
  kind: 'service',
  filters: { state: STATES },
  // One call serves every part that reads the services; the palette keeps what it gets for 30 s itself.
  items: async (cx) => itemsOf(await cx.cache('/api/1/services', 5000, () => cx.api.get('/api/1/services'))),
}];

export const phrases = {
  'service.start': 'Start {name}',
  'service.stop': 'Stop {name}',
  'service.restart': 'Restart {name}',
  'service.enable': 'Enable {name}',
  'service.disable': 'Disable {name}',
};
