// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the account's part of the Overview (design 3.5) makes of `GET
// /api/1/account/second-step`: `{app, passkeys, recovery, trusted}`, where
// `app` is null without an authenticator app and `passkeys` is a list.

/** Whether the account has a second step: an authenticator app or at least one passkey. */
export const hasSecondStep = (steps) => Boolean(steps?.app) || (Array.isArray(steps?.passkeys) && steps.passkeys.length > 0);

/** The needs of the Overview: an administrator whose account has no second step is told to add one. */
export function needsOf(session, steps) {
  if (!session.admin || hasSecondStep(steps)) return [];
  return [{
    id: 'account-second-step',
    severity: 'warn',
    title: 'Add a second step to your account',
    detail: 'Unlocking the console now asks for your password alone. An authenticator app or a passkey makes that safer.',
    href: '/second-step',
    icon: 'key',
    at: null,
  }];
}
