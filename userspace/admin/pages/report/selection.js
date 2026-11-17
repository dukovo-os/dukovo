// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Reads what is selected in the text of the report: the rows it covers, from
// which character to which in each, and where it ends. The one place that asks
// the browser for the selection; what it means for the cuts is cuts-model.js's.

/**
 * The selection inside `text`, the element that holds the rows (each a `.rl`
 * with `data-n` and a `.rl-text` cell): `{covered, top, left}`. `covered` is
 * `[{n, from, to}]`, the rows it touches with the UTF-16 offsets in each row's
 * text; `top` and `left` are where it ends from the top left of `frame`, in
 * pixels. Null when nothing inside `text` is selected.
 */
export function selectedIn(text, frame) {
  const selection = getSelection();
  if (!selection || selection.rangeCount === 0 || selection.isCollapsed) return null;
  const range = selection.getRangeAt(0);
  if (!text.contains(range.commonAncestorContainer)) return null;
  const covered = [];
  for (const cell of text.querySelectorAll('.rl-text')) {
    if (!range.intersectsNode(cell)) continue;
    const upTo = (node, offset) => {
      const part = document.createRange();
      part.selectNodeContents(cell);
      part.setEnd(node, offset);
      return part.toString().length;
    };
    const from = cell.contains(range.startContainer) ? upTo(range.startContainer, range.startOffset) : 0;
    const to = cell.contains(range.endContainer) ? upTo(range.endContainer, range.endOffset) : cell.textContent.length;
    if (to > from) covered.push({ n: Number(cell.parentElement.getAttribute('data-n')), from, to });
  }
  if (covered.length === 0) return null;
  const rects = range.getClientRects();
  const end = rects.length > 0 ? rects[rects.length - 1] : range.getBoundingClientRect();
  const where = frame.getBoundingClientRect();
  return { covered, top: end.bottom - where.top, left: end.right - where.left };
}
