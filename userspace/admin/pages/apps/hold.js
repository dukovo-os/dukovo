// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Hold at 8.0.3 on an app's page: the small question that asks why before the
// app is held, which everyone who can sign in reads with the hold. Read when
// the button is pressed.

import { openHold as ask } from '../../lib/hold-dialog.js';
import { holdLead } from './app-model.js';
import { holdCall, runWords } from './run-model.js';

/** Opens the question for the app of `info` (`bundle info --json`). Resolves with whether the app was held. */
export function openHold(view, { info }) {
  return ask(view, {
    title: `Hold ${info.name} at ${info.version}`,
    lead: holdLead(info),
    place: 'bundle list',
    call: (why) => holdCall(info.id, false, why),
    words: runWords('hold', info.name, info.version),
  });
}
