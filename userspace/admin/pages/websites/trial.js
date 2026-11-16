// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The banner of the one change these pages try: a port the firewall closes,
// opened for a while so that a website can be reached. The person keeps the
// rule once the site is live, or undoes it; the firewall puts the old rules
// back by itself when the time is out. The pages read this file when a port
// is opened or when a trial may already run.

import { makeTrialBanner } from '../../lib/trial-banner.js';

/** The banner, which calls `onEnd()` when a trial that ran has ended. Gives `{element, begin, read}`. */
export function makeFirewallTrial(view, { onEnd = () => {} } = {}) {
  const banner = makeTrialBanner(view, {
    id: 'firewall',
    what: () => 'A port is open for a while so that the website can be reached. Keep it once the website is live, or undo it now.',
    keep: 'firewall.keep',
    undo: 'firewall.undo',
    onState: ({ ended }) => {
      if (ended) onEnd();
    },
  });
  return { element: banner.element, begin: banner.begin, read: banner.read };
}
