// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The notice that the unlock is about to end, with the button that extends
// it. The shell loads this only when the end first comes near, so the
// start of a page carries none of it: most unlocks are ended by the person
// before the notice is due.

import { setUnlockSeconds } from '../unlock.js';

/** Says that the console locks in `minutes`; `post` and `notice` are the shell's. */
export function warnOfLock({ minutes, post, notice }) {
  notice(`The console locks in ${minutes} ${minutes === 1 ? 'minute' : 'minutes'}`, {
    kind: 'warn',
    action: {
      label: 'Reset to 15 min',
      run: () => post('/api/1/unlock/extend').then((answer) => setUnlockSeconds(answer.unlock_seconds), (error) => notice(error.title, { kind: 'warn' })),
    },
  });
}
