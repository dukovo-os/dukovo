// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Services (design 4): /services is the list, /services/scheduled the
// scheduled tasks and /services/NAME one service. Each is read when it is
// asked for, so that nothing of the others loads with it.

export async function show(view) {
  // A last slash is no part of the name.
  view.rest = view.rest.replace(/\/$/, '');
  if (view.rest === '') return (await import('./list.js')).show(view);
  if (view.rest === 'scheduled') return (await import('./scheduled.js')).show(view);
  return (await import('./detail.js')).show(view);
}
