// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the palette finds of the hardware: the check, with the state of its
// last result to filter by (`state:warning`), and, for an administrator, the
// restart and the shut down, each of which opens its card to ask first.

// The states the last result is in, the ones to look at first.
export const STATES = ['failed', 'warning', 'passed'];

/** The rows for the last check `last` (`GET /api/1/hardware/check`) and whether `admin` may restart. */
export function itemsOf(last, admin) {
  const checks = Array.isArray(last?.checks) ? last.checks : [];
  const state = STATES.find((word) => checks.some((one) => one.result === word)) ?? null;
  return [
    {
      key: 'check', title: 'Check the hardware', detail: state ? `The last check ${state === 'passed' ? 'passed' : `has ${state === 'failed' ? 'a failure' : 'a warning'}`}` : 'Not checked yet', state, href: '/hardware#check',
    },
    ...(admin ? [
      { key: 'restart', title: 'Restart this machine…', detail: 'It restarts a minute after you ask', href: '/hardware#power' },
      { key: 'shutdown', title: 'Shut down this machine…', detail: 'It stays off until someone turns it on', href: '/hardware#power' },
    ] : []),
  ];
}

export const providers = [{
  group: 'Hardware',
  kind: 'hardware',
  filters: { state: STATES },
  items: async (cx) => itemsOf(await cx.cache('/api/1/hardware/check', 5000, () => cx.api.get('/api/1/hardware/check').catch(() => null)), cx.session?.admin === true),
}];
