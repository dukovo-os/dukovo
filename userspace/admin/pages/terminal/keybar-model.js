// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The key bar of a phone's terminal (design 13.10): the keys a soft keyboard
// lacks. Ctrl and Alt are sticky: pressed, they wait for the next key,
// whichever way it comes (a bar key, or a key of the keyboard), and are
// spent by it; pressed again before that, they go.

import { keyBytes } from './term/keys.js';

/** The keys, in order: `id`, the words on the key and the key's name for a screen reader. */
export const KEYS = [
  { id: 'Escape', label: 'Esc', name: 'Escape' },
  { id: 'Tab', label: 'Tab', name: 'Tab' },
  { id: 'ctrl', label: 'Ctrl', name: 'Ctrl, for the next key', sticky: true },
  { id: 'alt', label: 'Alt', name: 'Alt, for the next key', sticky: true },
  { id: 'ArrowLeft', label: '\u2190', name: 'Left arrow' },
  { id: 'ArrowUp', label: '\u2191', name: 'Up arrow' },
  { id: 'ArrowDown', label: '\u2193', name: 'Down arrow' },
  { id: 'ArrowRight', label: '\u2192', name: 'Right arrow' },
  { id: '|', label: '|', name: 'Vertical bar' },
  { id: '/', label: '/', name: 'Slash' },
  { id: '~', label: '~', name: 'Tilde' },
  { id: '-', label: '-', name: 'Minus' },
];

const MODIFIERS = new Set(['Shift', 'Control', 'Alt', 'AltGraph', 'Meta', 'CapsLock', 'Dead', 'Process']);

export class KeyBar {
  ctrl = false;

  alt = false;

  /** Called when a waiting sticky key is spent or let go, so that the bar shows it no longer waits. */
  changed = null;

  /** Presses the key `id` of KEYS: a sticky key is toggled and gives null, another gives its bytes. */
  press(id, modes = {}) {
    if (id === 'ctrl' || id === 'alt') {
      this[id] = !this[id];
      return null;
    }
    return this.keyed({ key: id, code: '' }, modes);
  }

  /** The bytes of `event` (a key event, or `{key}`) with the sticky keys put on, which are then spent. */
  keyed(event, modes = {}) {
    if (MODIFIERS.has(event.key)) return null;
    const bytes = keyBytes({
      key: event.key, code: event.code ?? '', shiftKey: Boolean(event.shiftKey), metaKey: Boolean(event.metaKey),
      isComposing: Boolean(event.isComposing), ctrlKey: Boolean(event.ctrlKey) || this.ctrl, altKey: Boolean(event.altKey) || this.alt,
    }, modes);
    if (bytes !== null) this.clear();
    return bytes;
  }

  /** What a soft keyboard's text becomes while a sticky key waits: its bytes, or null when none waits. */
  typed(text, modes = {}) {
    if (!this.ctrl && !this.alt) return null;
    const first = [...text][0];
    return first === undefined ? null : this.keyed({ key: first }, modes);
  }

  /** Whether a sticky key waits. */
  get armed() {
    return this.ctrl || this.alt;
  }

  clear() {
    const waited = this.armed;
    this.ctrl = false;
    this.alt = false;
    if (waited) this.changed?.();
  }
}
