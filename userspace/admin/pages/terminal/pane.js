// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// One pane (design 13.10): its screen, what draws it, and what a person does
// at it: keys, pastes and input methods through a hidden text area that
// holds focus, the mouse (a selection, or reports for a program that asked
// for it), the wheel, touch, and the size it takes from its room. What it
// makes goes to `on`: `input(pane, bytes)`, `resize(pane, cols, rows)`,
// `action(name, pane)`, `menu(pane)`, `ask(pane, words)`, `bell(pane)`,
// `output(pane, bytes)`, `focus(pane)`, `keys()` (the key bar's sticky keys,
// when it has been made) and `said(words)`. Selecting, the mouse and pasting
// are read when first used, and so is the keys' file: a pane that has focus
// has it read, and a key that comes before it is ready waits for it.

import { h } from '../../dom.js';
import { menu } from '../../shell/menus.js';
import { Renderer } from './render.js';
import { Screen } from './term/screen.js';
import { fitOf } from './workspace-model.js';

const encoder = new TextEncoder();

export class Pane {
  selection = null;

  ended = false;

  attached = false;

  since = null;

  // The offset of the first byte of output written after it was attached.
  live = 0;

  #wheel = 0;

  #touch = 0;

  keyLib = null;

  #libs = null;

  #keyFile = null;

  /** `info` is `{id, terminal, admin, cols, rows}`; `settings` is what `configure` takes. */
  constructor(info, on, settings) {
    Object.assign(this, { id: info.id, terminal: info.terminal, admin: info.admin, on });
    this.screen = new Screen({
      cols: info.cols, rows: info.rows, scrollback: settings.scrollback, onBell: () => on.bell(this), onTitle: (title) => { this.title.textContent = title; },
    });
    this.renderer = new Renderer(this.screen);
    this.renderer.onDraw = () => { this.newest.hidden = this.renderer.back === 0; };
    this.name = h('span', { class: 'tr-name' });
    this.words = h('span', { class: 'tr-admin' });
    this.reset = h('button', { class: 'tr-reset', type: 'button', hidden: true, onclick: () => on.extend() }, 'Reset to 15 min');
    this.title = h('span', { class: 'tr-title' });
    this.newest = h('button', { class: 'tr-newest', type: 'button', hidden: true, onclick: () => this.renderer.scrollBy(-1e9) }, 'Newest \u2193');
    const more = menu({ label: 'Pane menu', content: ['\u22ef'], items: () => on.menu(this) });
    more.button.classList.add('tr-more');
    more.button.setAttribute('aria-label', 'Pane menu');
    this.input = h('textarea', {
      class: 'tr-input', rows: '1', autocapitalize: 'off', autocomplete: 'off', autocorrect: 'off', spellcheck: 'false',
    });
    this.note = h('div', { class: 'tr-note', role: 'status', hidden: true });
    this.body = h('div', { class: 'tr-body' }, this.renderer.element, this.input);
    this.element = h('section', { class: `tr-pane${info.admin ? ' tr-root' : ''}`, role: 'group', 'data-terminal': '' },
      h('header', { class: 'tr-head' }, this.name, this.words, this.reset, this.title, this.newest, more.element), this.body, this.note);
    this.configure(settings);
    this.#listen();
  }

  /** Takes the preferences a pane uses: `size` in pixels, `scrollback` lines, `shortcuts` and `copyOnSelect`. */
  configure({
    size, scrollback, shortcuts, copyOnSelect,
  }) {
    Object.assign(this, { shortcuts, copyOnSelect });
    if (scrollback !== this.screen.scrollbackLimit) this.screen.setScrollback(scrollback);
    if (size !== this.size) {
      this.size = size;
      this.renderer.setFont(size);
      this.sized = false;
    }
  }

  /** Names the pane for a screen reader and its head. */
  label({ title, index, count }) {
    this.element.setAttribute('aria-label', `Pane ${index + 1} of ${count}: ${title}${this.admin ? ', administrator' : ''}`);
    this.input.setAttribute('aria-label', `Terminal input, pane ${index + 1}`);
    this.name.textContent = `${title} \u00b7 ${index + 1}`;
  }

  /**
   * Output from the program: to the screen, and what the screen answers goes
   * back when `answer` is true. Output written before the page attached (a
   * replay, or what it missed) asked its questions of no one here: its
   * program has had an answer or stopped waiting, and an answer now would
   * be read as the answer to a later question.
   */
  write(bytes, answer = true) {
    const reply = this.screen.write(bytes);
    if (answer && reply.length > 0) this.on.answer(this, reply);
    if (this.selection) this.#select(this.sel.clipSelection(this.screen, this.selection));
    this.renderer.schedule();
    this.on.output(this, bytes);
  }

  /** A replay starts: the screen is new, with the modes admind says the program had set. */
  begin(modes = '') {
    this.screen.reset({ clearScrollback: true });
    this.#select(null);
    if (modes !== '') this.screen.write(encoder.encode(modes));
    this.renderer.resize();
  }

  /** Takes the size of its room; true when the screen changed size and the socket was told. */
  fit() {
    const { body, screen, renderer } = this;
    const box = getComputedStyle(body);
    const width = body.clientWidth - parseFloat(box.paddingLeft) - parseFloat(box.paddingRight);
    const height = body.clientHeight - parseFloat(box.paddingTop) - parseFloat(box.paddingBottom);
    if (!(width > 0 && height > 0)) return false;
    if (!this.sized) {
      renderer.measure();
      this.sized = true;
    }
    const { cols, rows } = fitOf(width, height, renderer.cell);
    if (cols === screen.cols && rows === screen.rows) return false;
    screen.resize(cols, rows);
    renderer.resize();
    this.#select(null);
    this.on.resize(this, cols, rows);
    return true;
  }

  focus() {
    this.input.focus({ preventScroll: true });
  }

  /** Marks the pane as ended, with words that say why. */
  end(words) {
    this.ended = true;
    this.note.textContent = words;
    this.note.hidden = false;
    this.element.classList.add('tr-over');
  }

  flash() {
    this.element.classList.add('tr-bell');
    setTimeout(() => this.element.classList.remove('tr-bell'), 500);
  }

  /** The text of the selection, or ''. */
  get selected() {
    return this.selection ? this.sel.selectionText(this.screen, this.selection) : '';
  }

  /** Copies the selection and ends it; gives words that say what happened, or null when nothing was selected. */
  async copy() {
    const text = this.selected;
    if (text === '') return null;
    this.#select(null);
    try {
      await navigator.clipboard.writeText(text);
      return 'Copied.';
    } catch {
      return 'The browser did not let the page copy. Read the screen has the text to copy.';
    }
  }

  /** Pastes `text`, or the clipboard's when it is not given. */
  async paste(text) {
    let words = text;
    if (words === undefined) {
      try {
        words = await navigator.clipboard.readText();
      } catch {
        this.on.said('The browser did not let the page paste. Use Ctrl+Shift+V.');
        return;
      }
    }
    const { pasteOf } = await import('./paste-model.js');
    const made = pasteOf(words, this.screen.bracketedPaste);
    if (made === null || this.ended) return;
    if (made.tooBig) this.on.said('That is too much to paste: 1 MB at most.');
    else if (!made.ask || (await this.on.ask(this, made.ask))) this.send(made.bytes);
  }

  clearScrollback() {
    this.screen.clearScrollback();
    this.renderer.scrollBy(-1e9);
    this.#select(null);
  }

  /** Types `bytes`: the view goes to the newest line, and a selection goes. */
  send(bytes) {
    this.renderer.scrollBy(-1e9);
    this.#select(null);
    this.on.input(this, bytes);
  }

  #select(selection) {
    this.selection = selection;
    this.renderer.selection = selection;
    this.renderer.schedule();
  }

  // Reads selecting and the mouse; ready by the time a button is pressed, as the pointer comes first.
  #warm() {
    this.#libs ??= Promise.all([import('./term/select.js'), import('./term/mouse.js')]).then(([sel, mouse]) => {
      this.sel = sel;
      this.reporter = new mouse.MouseReporter();
      this.renderer.span = (line) => sel.selectionSpan(this.screen, this.selection, line);
    });
    return this.#libs;
  }

  // Reads the keys' file once.
  #readKeys() {
    this.#keyFile ??= import('./term/keys.js').then((keys) => {
      this.keyLib = keys;
    });
    return this.#keyFile;
  }

  #listen() {
    const { input, body, screen } = this;
    input.addEventListener('keydown', (event) => this.#key(event));
    const flush = () => {
      const text = input.value.replace(/\n/g, '\r');
      input.value = '';
      if (text === '' || this.ended) return;
      const keys = this.on.keys();
      this.send(keys?.typed(text, screen) ?? encoder.encode(text));
      keys?.clear();
    };
    input.addEventListener('input', (event) => event.isComposing || flush());
    input.addEventListener('compositionend', flush);
    input.addEventListener('compositionstart', () => {
      input.style.transform = `translate(${screen.x * this.renderer.cell.w}px, ${screen.y * this.renderer.cell.h}px)`;
    });
    input.addEventListener('paste', (event) => {
      event.preventDefault();
      this.paste(event.clipboardData?.getData('text/plain') ?? '');
    });
    input.addEventListener('focus', () => {
      this.#readKeys();
      this.element.classList.add('tr-focused');
      this.renderer.focused = true;
      this.renderer.schedule();
      this.on.focus(this);
    });
    input.addEventListener('blur', () => {
      this.element.classList.remove('tr-focused');
      this.renderer.focused = false;
      this.renderer.schedule();
    });
    body.addEventListener('pointerenter', () => this.#warm(), { once: true });
    body.addEventListener('mousedown', (event) => this.#press(event));
    body.addEventListener('wheel', (event) => this.#scroll(event), { passive: false });
    body.addEventListener('touchstart', (event) => {
      this.#touch = event.touches[0].clientY;
    }, { passive: true });
    body.addEventListener('touchmove', (event) => {
      const y = event.touches[0].clientY;
      const lines = Math.trunc((y - this.#touch) / this.renderer.cell.h);
      if (lines !== 0 && !screen.mouseTracking) {
        this.renderer.scrollBy(lines);
        this.#touch = y;
      }
    }, { passive: true });
  }

  #key(event) {
    if (!this.keyLib) {
      // Before the file is read: the key waits, as a plain copy of what it was.
      event.preventDefault();
      const { key, code, ctrlKey, altKey, shiftKey, metaKey, isComposing } = event;
      this.#readKeys().then(() => this.#keyed({
        key, code, ctrlKey, altKey, shiftKey, metaKey, isComposing, preventDefault() {},
      }));
      return;
    }
    this.#keyed(event);
  }

  #keyed(event) {
    const { screen } = this;
    const { keyBytes, pageAction } = this.keyLib;
    const action = pageAction(event, this.shortcuts);
    if (action === 'paste' || action === 'palette') return;
    if (action) {
      event.preventDefault();
      this.on.action(action, this);
      return;
    }
    const plain = !event.ctrlKey && !event.altKey;
    if (event.ctrlKey && !event.shiftKey && !event.altKey && event.key === 'c' && this.selection) {
      event.preventDefault();
      this.copy().then((words) => this.on.said(words));
      return;
    }
    if (plain && event.shiftKey && !screen.alternate && (event.key === 'PageUp' || event.key === 'PageDown')) {
      event.preventDefault();
      this.renderer.scrollBy((event.key === 'PageUp' ? 1 : -1) * Math.max(screen.rows - 1, 1));
      return;
    }
    if (this.ended) return;
    const modes = { applicationCursor: screen.applicationCursor, applicationKeypad: screen.applicationKeypad };
    const keys = this.on.keys();
    const bytes = keys ? keys.keyed(event, modes) : keyBytes(event, modes);
    if (bytes !== null) {
      event.preventDefault();
      this.send(bytes);
    }
  }

  // A mouse report, for a program that asked for the mouse.
  #report(action, event, direction, button = event.button) {
    const cell = this.renderer.cellAt(event.clientX, event.clientY);
    const bytes = this.reporter?.report({
      action, direction, button, x: cell.col, y: cell.row, altKey: event.altKey, ctrlKey: event.ctrlKey, shiftKey: event.shiftKey,
    }, this.screen) ?? null;
    if (bytes !== null) this.on.input(this, bytes);
  }

  #press(event) {
    this.focus();
    // A program that asked for the mouse has it; Shift keeps the page's selecting.
    const reporting = this.screen.mouseTracking && !event.shiftKey;
    if (!reporting && event.button !== 0) return;
    event.preventDefault();
    const { detail, button } = event;
    const at = this.renderer.cellAt(event.clientX, event.clientY);
    // What follows the press is listened for at once; the files the work needs may still be on their way.
    let released = null;
    let onMove = null;
    let onUp = null;
    const move = (one) => onMove?.(one);
    const done = (one) => {
      released = one;
      document.removeEventListener('mousemove', move);
      document.removeEventListener('mouseup', done);
      onUp?.(one);
    };
    document.addEventListener('mousemove', move);
    document.addEventListener('mouseup', done);
    this.#warm().then(() => {
      if (reporting) {
        this.#report('press', event);
        onMove = (one) => this.#report('move', one, undefined, button);
        onUp = (one) => this.#report('release', one, undefined, button);
      } else if (detail >= 2) {
        this.#select(detail >= 3 ? this.sel.lineRange(this.screen, at.line) : this.sel.wordRange(this.screen, at.line, at.col));
      } else {
        this.#select(null);
        onMove = (one) => {
          const to = this.renderer.cellAt(one.clientX, one.clientY);
          this.#select(to.line === at.line && to.col === at.col ? null : { start: { line: at.line, col: at.col }, end: { line: to.line, col: to.col } });
        };
        onUp = () => {
          if (this.selection && this.copyOnSelect) this.copy().then(this.on.said);
        };
      }
      if (released) onUp?.(released);
    });
  }

  #scroll(event) {
    event.preventDefault();
    const { screen } = this;
    this.#wheel += event.deltaMode === 0 ? event.deltaY / this.renderer.cell.h : event.deltaY * (event.deltaMode === 1 ? 1 : screen.rows);
    const lines = Math.trunc(this.#wheel);
    if (lines === 0) return;
    this.#wheel -= lines;
    const count = Math.min(Math.abs(lines), 10);
    const direction = lines < 0 ? 'up' : 'down';
    if (screen.mouseTracking && !event.shiftKey) {
      this.#warm().then(() => {
        for (let i = 0; i < count; i++) this.#report('wheel', event, direction);
      });
    } else if (screen.alternate) {
      // A full-screen program that did not ask for the mouse: the wheel is its arrow keys.
      this.on.input(this, encoder.encode(`\x1b${screen.applicationCursor ? 'O' : '['}${lines < 0 ? 'A' : 'B'}`.repeat(count)));
    } else {
      this.renderer.scrollBy(-lines);
    }
  }
}
