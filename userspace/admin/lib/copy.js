// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Copying text to the clipboard from a button. A browser may refuse (the
// page is not in a secure context, the permission is off), and then the
// text is selected where the person can see it, so that Ctrl+C copies it.

/**
 * Copies `text` with the clipboard API and gives true. When the browser
 * refuses, selects the contents of `shown`, an element that holds the same
 * text, and gives false: the caller then says "Press Ctrl+C to copy".
 */
export async function copyText(text, shown) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    if (shown) {
      const range = document.createRange();
      range.selectNodeContents(shown);
      const selection = getSelection();
      selection.removeAllRanges();
      selection.addRange(range);
    }
    return false;
  }
}

/** The words that say a copy went through, and the ones for the copy that was left to the person. */
export const COPIED = 'Copied.';
export const PRESS_CTRL_C = 'Press Ctrl+C to copy.';
