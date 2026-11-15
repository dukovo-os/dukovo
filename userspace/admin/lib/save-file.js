// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Saving text as a file on the person's own machine (the Logs page's Save as
// file): a Blob, and a link with `download` that is clicked once. Nothing is
// sent anywhere, and nothing runs at import.

import { h } from '../dom.js';

/** How long the address of the saved text is kept before it is freed: the browser has begun the save by then. */
const KEEP_MS = 10_000;

/** Saves `text` as the file `name`, UTF-8 plain text. */
export function saveText(name, text) {
  const address = URL.createObjectURL(new Blob([text], { type: 'text/plain;charset=utf-8' }));
  // A link that is not in the page is not clicked by every browser.
  const link = h('a', { href: address, download: name, hidden: true });
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(address), KEEP_MS);
}
