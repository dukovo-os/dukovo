// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Showing a backup place's recovery key (design 5.7 and 5.8, ADR 0561): it
// is shown once, in the dialog of lib/shown-once.js, which drops it when it
// closes. Adding a place shows it for the first time and Show it again
// shows it later; both come here. The page reads this file when a key is
// first to be shown.

import { recoveryKeyOf } from './form-model.js';

/** What backup(8) says of the recovery key, beside it. */
const KEEP_WORDS = 'Keep it somewhere other than this machine. Without it and this machine, the backups cannot be read.';

/**
 * Shows the recovery key of the backup place `name` that `answer` (of
 * `backups.add` or `backups.key`) holds, once. `added` says it is the
 * answer of adding the place, for the words of the notice when the answer
 * holds no key. Gives true when the key was kept. Nothing of the key is
 * left here after it returns.
 */
export async function showRecoveryKey(view, { name, answer, added = false }) {
  let key = recoveryKeyOf(answer);
  if (key === null) {
    view.notice(added
      ? `The backup place ${name} was added, but the answer held no recovery key. Show it on the place's page.`
      : `The answer held no recovery key for ${name}.`, { kind: 'warn' });
    return false;
  }
  const { showOnce } = await import('../../lib/shown-once.js');
  const kept = await showOnce(view, {
    title: `Recovery key for ${name}`,
    label: 'Recovery key',
    text: key,
    file: `${name}-recovery.key`,
    warn: KEEP_WORDS,
    loss: `You can show it again on the page of ${name}, as an administrator.`,
  });
  key = null;
  return kept;
}
