// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The banners of the two changes this page tries: the certificate the console
// serves, which the person keeps once this page has come back on it, and the
// port 80 that is opened for an order. The page reads this file when it opens,
// to show a trial that runs, and when a change begins one.

import { makeTrialBanner } from '../../lib/trial-banner.js';
import { servingWords } from './row-model.js';

/** Makes the two banners for the page `ctx`. Gives `{console, firewall, elements, read}`. */
export function makeTrials(ctx) {
  const consoleBanner = makeTrialBanner(ctx.view, {
    id: 'console',
    what: () => (ctx.data.serving ? servingWords(ctx.data.serving) : 'The console\'s certificate is being tried.'),
    keep: 'console.keep',
    undo: 'console.undo',
    onState: ({ ended }) => {
      if (ended) ctx.reread();
    },
  });
  const firewallBanner = makeTrialBanner(ctx.view, {
    id: 'firewall',
    what: () => 'Port 80 is open for a while so that Let\'s Encrypt can reach this server. Keep it once the certificate is got, or undo it now.',
    keep: 'firewall.keep',
    undo: 'firewall.undo',
    onState: ({ ended }) => {
      if (ended) ctx.get?.check();
    },
  });
  return {
    console: consoleBanner,
    firewall: firewallBanner,
    elements: [consoleBanner.element, firewallBanner.element],
    read: () => Promise.all([consoleBanner.read(), firewallBanner.read()]),
  };
}
