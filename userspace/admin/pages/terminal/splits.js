// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What a terminal with more than one pane needs on the page (design 13.10):
// the separators that a pointer drags and the arrow keys move by 5%, and the
// Layout choices of the menu. The workspace reads this file when the first
// terminal with two panes is shown, so a person with one pane never loads it.

import { post } from '../../api.js';
import { h } from '../../dom.js';
import {
  LAYOUT_WORDS, LAYOUTS, layoutForSplit, movedSizes, neighbour, placeOf, sizeAt, sizesFor,
} from './layout-model.js';

export { layoutForSplit, neighbour, placeOf };

const percent = (n) => `${Math.round(n * 10000) / 100}%`;

/** The separator element for `separator` of `terminal`'s `layout`; `ws` is the workspace, which draws again and saves. */
export function separator(ws, terminal, one, layout) {
  const along = one.axis === 'x';
  const element = h('div', {
    class: `tr-sep tr-sep-${one.axis}`, role: 'separator', tabindex: '0', 'aria-label': 'Resize panes', 'aria-orientation': along ? 'vertical' : 'horizontal',
    'aria-valuemin': '20', 'aria-valuemax': '80', 'aria-valuenow': String(Math.round(one.size * 100)), 'data-index': String(one.index),
  });
  const { style } = element;
  style[along ? 'left' : 'top'] = percent(one.at);
  style[along ? 'top' : 'left'] = percent(one.across[0]);
  style[along ? 'height' : 'width'] = percent(one.across[1] - one.across[0]);
  const count = terminal.panes.length;
  element.addEventListener('keydown', (event) => {
    const step = { ArrowRight: 0.05, ArrowDown: 0.05, ArrowLeft: -0.05, ArrowUp: -0.05 }[event.key];
    if (step === undefined || along === /Up|Down/.test(event.key)) return;
    event.preventDefault();
    terminal.sizes = movedSizes(layout, count, terminal.sizes, one.index, step, true);
    ws.edits += 1;
    ws.drawPanes();
    ws.dom.grid.querySelector(`[data-index="${one.index}"]`)?.focus();
    clearTimeout(ws.saving);
    ws.saving = setTimeout(() => setLayout(ws, layout, terminal.sizes), 300);
  });
  element.addEventListener('pointerdown', (event) => {
    event.preventDefault();
    element.setPointerCapture(event.pointerId);
    const box = ws.dom.grid.getBoundingClientRect();
    const drag = (moved) => {
      const at = along ? (moved.clientX - box.left) / box.width : (moved.clientY - box.top) / box.height;
      terminal.sizes = movedSizes(layout, count, terminal.sizes, one.index, sizeAt(one, at));
      ws.edits += 1;
      ws.drawPanes();
    };
    const up = () => {
      element.removeEventListener('pointermove', drag);
      element.removeEventListener('pointerup', up);
      setLayout(ws, layout, terminal.sizes);
    };
    element.addEventListener('pointermove', drag);
    element.addEventListener('pointerup', up);
  });
  return element;
}

/**
 * Shows `layout` with `sizes` for the terminal shown, and saves them. A read
 * of the list that began before, or that ends while admind has not answered
 * yet, would put the old layout back for a moment: the workspace drops it
 * and reads again (`ws.edits`, `ws.changing`), and reads once more here when
 * admind has answered, which is what it keeps after a refusal too.
 */
export async function setLayout(ws, layout, sizes = []) {
  clearTimeout(ws.saving);
  ws.saving = null;
  const terminal = ws.terminal;
  if (!terminal) return;
  Object.assign(terminal, { layout, sizes });
  ws.edits += 1;
  ws.changing += 1;
  ws.draw();
  try {
    await post(`/api/1/terminals/${terminal.id}/layout`, { layout, sizes });
  } catch (error) {
    ws.view?.problem(error);
  } finally {
    ws.changing -= 1;
  }
  ws.later();
}

/** Before a split, saves what a separator waits to save, which would put back the old layout. */
export async function flush(ws) {
  if (ws.saving) await setLayout(ws, ws.terminal.layout, ws.terminal.sizes);
}

/** The menu items for the Layout choices of `terminal`, and for the share of its first split. */
export function layoutItems(ws, terminal) {
  const count = terminal.panes.length;
  const items = [{ separator: true }, { heading: 'Layout' }];
  for (const layout of LAYOUTS) items.push({ label: LAYOUT_WORDS[layout], radio: true, checked: terminal.layout === layout, onSelect: () => setLayout(ws, layout) });
  const sizes = sizesFor(terminal.layout, count, terminal.sizes);
  if (sizes.length > 0) {
    items.push({ separator: true }, { heading: 'First split' });
    for (const share of [30, 50, 70]) {
      items.push({
        label: `${share}%`, radio: true, checked: Math.round(sizes[0] * 100) === share, onSelect: () => setLayout(ws, terminal.layout, movedSizes(terminal.layout, count, terminal.sizes, 0, share / 100)),
      });
    }
  }
  return items;
}
