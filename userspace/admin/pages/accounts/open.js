// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The way the sections open a dialog: dialog.js, the shell's dialogs and what
// they need are read when the first dialog is asked for, since most visits
// to an account change nothing.

/** Opens the dialog `options` describe (see dialog.js) and resolves with whether a change was made. */
export async function openForm(view, options) {
  const { openForm: open } = await import('./dialog.js');
  return open(view, options);
}
