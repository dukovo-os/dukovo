// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Showing a database's password once: in the dialog of lib/shown-once.js, with
// the connection settings and Copy and Save as file, which drops the text when
// it closes. Add a database shows the first one and a database's page shows a
// new one; both come here, and this file is read when a password is first to be
// shown.

import { fileName, settingsFile } from '../../lib/database-model.js';

/** What a person is told when the password is lost: the way back. */
const LOST = 'Lost, it is replaced by a new one on the database\'s page.';

/**
 * Shows the password of `secret` (`secretOf`'s) for the database `name`
 * once, in the settings of `way` (see `settingsFile`), and gives whether
 * it was kept. Nothing of the password is left here when it returns.
 */
export async function showPassword(view, {
  name, secret, way, machine,
}) {
  const { showOnce } = await import('../../lib/shown-once.js');
  let text = settingsFile({
    way, database: name, role: secret.role, password: secret.password, machine,
  });
  const kept = await showOnce(view, {
    title: `The password of ${name}`,
    label: 'Password and settings',
    text,
    file: fileName(name),
    warn: `It is shown only now: copy it or save it. ${LOST}`,
    loss: `The password is shown only now. ${LOST}`,
  });
  text = null;
  return kept;
}
