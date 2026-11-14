// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A screen as DOM (design 13.10): a row of spans for each line shown, from
// render-model.js's runs and term.css's classes, redrawn only when its look
// changed, at the next frame so that a flood of output draws once. Rows are
// `aria-hidden`: a screen reader uses Read the screen and the live region.
// The view can lie above the newest lines: `top` is the number of the line
// at its top (null follows the screen), and a line keeps its number while
// output scrolls.

import { h } from '../../dom.js';
import { rowRuns } from './render-model.js';

const SHAPES = ['block', 'underline', 'bar'];

// contrast-model.js, loaded at the first draw, draws hard-to-read text
// readable, and the screens that draw again when the theme changes.
const screens = new Set();
let readable = null;
let loading = null;

// The first and last line of a selection kept as JSON, or null.
function linesOf(json) {
  if (json === '') return null;
  const { start, end } = JSON.parse(json);
  return [Math.min(start.line, end.line), Math.max(start.line, end.line)];
}

const within = (n, lines) => lines !== null && n >= lines[0] && n <= lines[1];

export class Renderer {
  top = null;

  selection = null;

  span = null;

  focused = false;

  cell = { w: 8, h: 16 };

  onDraw = null;

  #rows = [];

  #drawn = { base: null, cursor: '', selection: '' };

  #frame = 0;

  #epoch = 0;

  constructor(screen) {
    this.screen = screen;
    this.element = h('div', { class: 'tr-screen' });
    this.#epoch = screen.epoch;
    this.resize();
  }

  /** Makes as many rows as the screen has. */
  resize() {
    this.#rows = Array.from({ length: this.screen.rows }, () => ({ el: h('div', { class: 'tr-row', 'aria-hidden': 'true' }), key: null }));
    this.element.replaceChildren(...this.#rows.map((row) => row.el));
    this.top = null;
    this.refresh();
  }

  /** Draws every row again at the next frame. */
  refresh() {
    this.#drawn.base = null;
    this.schedule();
  }

  /** Sets the text size in pixels; a line is a quarter more. `measure` follows once the pane is in the page. */
  setFont(px) {
    this.cell = { w: this.cell.w, h: Math.round(px * 1.25) };
    this.element.style.fontSize = `${px}px`;
    this.element.style.setProperty('--tr-h', `${this.cell.h}px`);
  }

  /** Measures the width of a cell from twenty characters of the font in use. */
  measure() {
    const probe = h('span', { class: 'tr-probe' }, '0'.repeat(20));
    this.#rows[0].el.append(probe);
    const { width } = probe.getBoundingClientRect();
    probe.remove();
    if (width > 0) this.cell = { w: width / 20, h: this.cell.h };
    this.refresh();
  }

  /** How many lines the view is above the newest. */
  get back() {
    return this.top === null ? 0 : this.screen.scrolled - this.top;
  }

  /** Moves the view up by `lines` (down when negative); at the newest line it follows the screen again. */
  scrollBy(lines) {
    const { screen } = this;
    const next = Math.min(Math.max((this.top ?? screen.scrolled) - lines, screen.firstLine), screen.scrolled);
    this.top = next >= screen.scrolled ? null : next;
    this.schedule();
  }

  /** The cell under a point of the page: its column and row from 0 at the top left, and the number of its line. */
  cellAt(x, y) {
    const box = this.element.getBoundingClientRect();
    const { cols, rows } = this.screen;
    const col = Math.min(Math.max(Math.floor((x - box.left) / this.cell.w), 0), cols - 1);
    const row = Math.min(Math.max(Math.floor((y - box.top) / this.cell.h), 0), rows - 1);
    return { col, row, line: this.#base() + row };
  }

  #base() {
    const { screen } = this;
    return Math.min(Math.max(this.top ?? screen.scrolled, screen.firstLine), screen.scrolled);
  }

  schedule() {
    screens.add(this);
    if (this.#frame !== 0) return;
    this.#frame = requestAnimationFrame(() => {
      this.#frame = 0;
      this.draw();
    });
  }

  /** Draws the rows that changed. */
  draw() {
    const { screen } = this;
    if (!this.element.isConnected) {
      screens.delete(this);
      return;
    }
    loading ??= import('./contrast-model.js').then((module) => {
      readable = module.follow(() => screens.forEach((one) => one.refresh()));
    }).catch(console.error);
    if (screen.epoch !== this.#epoch) {
      this.#epoch = screen.epoch;
      this.top = null;
    }
    if (this.#rows.length !== screen.rows) this.resize();
    const base = this.#base();
    const live = base === screen.scrolled;
    const cursorLine = live && screen.cursorVisible ? screen.scrolled + screen.y : -1;
    const cursor = cursorLine < 0 ? '' : `${cursorLine},${screen.x}`;
    const selection = this.selection ? JSON.stringify(this.selection) : '';
    const before = this.#drawn;
    const all = before.base !== base || (!live && screen.dirty.includes(1));
    const left = before.cursor === cursor ? -1 : Number(before.cursor.split(',')[0]);
    const changed = before.selection !== selection;
    const was = changed ? linesOf(before.selection) : null;
    const now = changed ? linesOf(selection) : null;
    this.#rows.forEach((row, y) => {
      const number = base + y;
      if (!all && !screen.dirty[y] && number !== cursorLine && number !== left && !within(number, was) && !within(number, now)) return;
      const span = this.selection && this.span ? this.span(number) : null;
      let runs = rowRuns(screen, number, { span, cursor: number === cursorLine ? screen.x : -1 });
      if (readable) runs = readable(runs, this.element);
      const key = JSON.stringify(runs);
      if (key === row.key) return;
      row.key = key;
      row.el.replaceChildren(...runs.map(({
        text, cls, color, background,
      }) => {
        if (cls === '' && color === '' && background === '') return document.createTextNode(text);
        const run = h('span', { class: cls || null }, text);
        if (color) run.style.color = color;
        if (background) run.style.backgroundColor = background;
        return run;
      }));
    });
    screen.clearDirty();
    this.#drawn = { base, cursor, selection };
    const { classList } = this.element;
    const shape = SHAPES.includes(screen.cursorShape) ? screen.cursorShape : 'block';
    for (const one of SHAPES) classList.toggle(`tr-${one}`, one === shape);
    classList.toggle('tr-blink', screen.cursorBlink);
    classList.toggle('tr-focus', this.focused);
    this.onDraw?.();
  }
}
