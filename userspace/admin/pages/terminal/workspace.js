// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Terminal page's workspace (design 13.10): the person's terminals as
// tabs, the panes of the one shown, the key bar, the footer and the socket.
// page.js keeps it for a while after the person leaves, so that the screens
// and the socket stay and only the page's elements are made again.
// Splits, the key bar, the voice and the dialogs are read when first used.

import { get, post } from '../../api.js';
import { h, icon } from '../../dom.js';
import { menu } from '../../shell/menus.js';
import { onUnlockChange, setUnlockSeconds, unlockMinutes } from '../../unlock.js';
import { kept as shown } from './kept.js';
import { Pane } from './pane.js';
import { statusWords } from './reconnect-model.js';
import { openSocket } from './socket.js';
import {
  adminWords, attachPlan, closesAtOnce, endedWords, nearsEnd, sizeStep, tabAfter, tabName, withKept,
} from './workspace-model.js';

const API = '/api/1/terminals';
const PHONE = '(max-width: 51.4rem)';
const WHOLE = { x: 0, y: 0, w: 1, h: 1 };
const percent = (n) => `${Math.round(n * 10000) / 100}%`;
const button = (attributes, ...children) => h('button', { type: 'button', ...attributes }, ...children);
const tool = (attributes, ...children) => button({ class: 'btn small', ...attributes }, ...children);

export class Workspace {
  terminals = [];

  listed = [];

  current = null;

  focused = null;

  panes = new Map();

  ghosts = new Map();

  activity = new Set();

  status = { state: 'connecting', attempt: 0 };

  prefs = {};

  motion = 'full';

  keys = null;

  voice = null;

  splits = null;

  view = null;

  dom = null;

  constructor(session) {
    this.session = session;
    this.on = {
      input: (pane, bytes) => this.socket.input(pane.id, bytes),
      answer: (pane, bytes) => this.socket.answer(pane.id, bytes),
      resize: (pane, cols, rows) => this.socket.resize(pane.id, cols, rows),
      action: (name, pane) => this.act(name, pane),
      menu: (pane) => this.paneMenu(pane),
      ask: (pane, words) => this.confirm('Paste?', words, 'Paste'),
      bell: (pane) => this.bell(pane),
      output: (pane, bytes) => this.output(pane, bytes),
      focus: (pane) => this.focusChanged(pane),
      keys: () => this.keys,
      extend: () => this.extend(),
      said: (words) => words && this.view?.notice(words, { kind: 'info' }),
    };
    this.socket = openSocket({
      location,
      token: session.csrf,
      onStatus: (status) => this.statusChanged(status),
      onChanged: () => this.later(),
      onProblem: (words) => this.view?.notice(words, { kind: 'warn' }),
    });
  }

  // ---- showing ----

  /** Shows the workspace in `view` and brings the terminals up to date. */
  async mount(view) {
    this.view = view;
    this.takePrefs(view.prefs);
    for (const pane of this.panes.values()) pane.configure(this.settings);
    const phone = matchMedia(PHONE);
    const resized = new ResizeObserver(() => {
      clearTimeout(this.fitting);
      this.fitting = setTimeout(() => this.fit(), 80);
    });
    const visible = () => document.visibilityState === 'visible' && this.socket.retryNow();
    const changed = () => {
      this.phone = phone.matches;
      this.draw();
    };
    this.phone = phone.matches;
    phone.addEventListener('change', changed);
    document.addEventListener('visibilitychange', visible);
    this.cleanups = [
      () => phone.removeEventListener('change', changed),
      () => document.removeEventListener('visibilitychange', visible),
      () => resized.disconnect(),
      onUnlockChange(() => this.adminWords()),
      view.onPrefs((prefs) => {
        this.takePrefs(prefs);
        for (const pane of this.panes.values()) pane.configure(this.settings);
        this.draw();
      }),
    ];
    view.main.classList.add('tr-page');
    this.build();
    resized.observe(this.dom.grid);
    this.drawStatus();
    await this.refresh();
  }

  /** Takes the elements away; the screens and the socket stay. */
  unmount() {
    for (const cleanup of this.cleanups) cleanup();
    clearTimeout(this.fitting);
    this.voice?.stop();
    this.dom = null;
    this.view = null;
  }

  /** Ends the socket and forgets the screens. */
  dispose() {
    this.socket.stop();
    this.panes.clear();
    shown.panes.clear();
  }

  get terminal() {
    return this.terminals.find((one) => one.id === this.current) ?? null;
  }

  get settings() {
    const { prefs } = this;
    return {
      size: prefs.size ?? 14, scrollback: prefs.scrollback ?? 10000, shortcuts: prefs.shortcuts !== false, copyOnSelect: prefs['copy-on-select'] === true,
    };
  }

  takePrefs(prefs) {
    this.prefs = prefs.terminal ?? {};
    this.motion = prefs.motion;
  }

  // ---- the terminals admind lists ----

  /** Reads the list and brings the panes, the socket and the page in line with it. */
  async refresh() {
    let answer;
    try {
      answer = await get(API);
    } catch (error) {
      this.view?.problem(error);
      return;
    }
    this.listed = answer.terminals ?? [];
    this.terminals = withKept(this.listed, this.ghosts);
    if (!this.terminal) this.current = this.terminals[0]?.id ?? null;
    this.sync();
    this.draw();
  }

  later() {
    clearTimeout(this.refreshing);
    this.refreshing = setTimeout(() => this.refresh(), 150);
  }

  // A Pane for every pane listed, and the socket showing as many as it may.
  sync() {
    const wanted = new Map(this.terminals.flatMap((terminal) => terminal.panes.map((info) => [info.id, { info, terminal }])));
    for (const [id, pane] of this.panes) {
      if (wanted.has(id)) continue;
      this.detach(pane);
      this.panes.delete(id);
      pane.element.remove();
    }
    for (const [id, { info, terminal }] of wanted) {
      if (!this.panes.has(id)) this.panes.set(id, new Pane({ ...info, terminal: terminal.id }, this.on, this.settings));
    }
    const plan = new Set(attachPlan(this.terminals, this.current));
    for (const [id, pane] of this.panes) {
      if (plan.has(id) && !pane.attached && !pane.ended) this.attach(pane);
      else if (!plan.has(id) && pane.attached) this.detach(pane);
    }
  }

  attach(pane) {
    pane.attached = true;
    shown.panes.add(pane.id);
    this.socket.attach(pane.id, { cols: pane.screen.cols, rows: pane.screen.rows, since: pane.since }, {
      attached: (message) => {
        if (message.replay) pane.begin(message.modes);
        pane.since = message.offset;
        pane.live = message.live;
      },
      output: (bytes, offset) => {
        pane.write(bytes, offset >= pane.live);
        pane.since = offset + bytes.length;
      },
      ended: (message) => this.paneEnded(pane, message),
      detached: () => this.attach(pane),
      error: (error) => {
        if (error.id === 'ADM-131') this.later();
        else this.view?.notice(`${error.title}. ${error.detail}`, { kind: 'warn' });
      },
    });
  }

  detach(pane) {
    if (!pane.attached) return;
    pane.attached = false;
    shown.panes.delete(pane.id);
    this.socket.detach(pane.id);
  }

  // A pane that ended: it goes at once when the program ended or someone ended it, else it stays with the words that say why.
  paneEnded(pane, message) {
    pane.attached = false;
    shown.panes.delete(pane.id);
    const terminal = this.terminals.find((one) => one.id === pane.terminal);
    if (!closesAtOnce(message.why) && terminal) {
      pane.end(endedWords(message.why, message.status));
      const old = this.ghosts.get(pane.terminal) ?? { ...terminal, panes: [] };
      this.ghosts.set(pane.terminal, { ...old, panes: [...old.panes, ...terminal.panes.filter((one) => one.id === pane.id)] });
    }
    this.later();
  }

  statusChanged(status) {
    this.status = status;
    this.drawStatus();
    if (status.state === 'open') this.later();
    else if (status.refused && status.attempt === 1) this.explain();
  }

  // A browser hides why a socket was refused: admind's own answer to the list says it.
  async explain() {
    try {
      const { sockets } = await get(API);
      if (sockets && sockets.open >= sockets.most) this.view?.notice(`This browser has the Terminal open in ${sockets.open} tabs, as many as it may. Close one.`, { kind: 'warn' });
    } catch (error) {
      this.view?.problem(error);
    }
  }

  // ---- what the person does ----

  /** The size a new pane asks for: that of the pane that has focus. */
  room() {
    const pane = this.panes.get(this.focused) ?? [...this.panes.values()][0];
    return { cols: pane?.screen.cols ?? 80, rows: pane?.screen.rows ?? 24 };
  }

  async newTerminal({ admin = false } = {}) {
    try {
      const { terminal } = await post(API, { ...this.room(), admin, keep_days: this.prefs['keep-days'] });
      this.current = terminal.id;
      await this.opened(terminal);
    } catch (error) {
      this.view?.problem(error, { retry: () => this.newTerminal({ admin }) });
    }
  }

  async split(direction) {
    const terminal = this.terminal;
    if (!terminal) return this.newTerminal();
    if (terminal.panes.length >= 4) return this.view.notice('A terminal has 4 panes at most.', { kind: 'info' });
    try {
      const splits = await this.needSplits();
      await splits.flush(this);
      const layout = splits.layoutForSplit(terminal.layout, terminal.panes.length, direction);
      await this.opened((await post(`${API}/${terminal.id}/panes`, { ...this.room(), layout })).terminal);
    } catch (error) {
      this.view?.problem(error);
    }
    return undefined;
  }

  // A new pane or terminal is listed, shown and given focus.
  async opened(terminal) {
    this.focused = terminal.panes.at(-1)?.id ?? this.focused;
    await this.refresh();
    this.panes.get(this.focused)?.focus();
  }

  async needSplits() {
    this.splits ??= await import('./splits.js');
    return this.splits;
  }

  async setLayout(layout, sizes = []) {
    clearTimeout(this.saving);
    this.saving = null;
    const terminal = this.terminal;
    if (!terminal) return;
    Object.assign(terminal, { layout, sizes });
    this.draw();
    try {
      await post(`${API}/${terminal.id}/layout`, { layout, sizes });
    } catch (error) {
      this.view?.problem(error);
      this.refresh();
    }
  }

  async confirm(title, words, yes) {
    const { ask } = await import('../../shell/dialogs.js');
    return (await ask({ title, words, iconName: 'warn', choices: [{ label: yes, value: 'yes', kind: 'default' }, { label: 'Cancel', value: 'no', safe: true }] })) === 'yes';
  }

  // Ending a pane or a terminal, and taking off one that ended and stayed, is read when first needed.
  async ending(name, thing) {
    return (await import('./ending.js'))[name](this, thing);
  }

  endPane(pane) {
    return this.ending('endPane', pane);
  }

  closeTab(terminal) {
    return this.ending('closeTab', terminal);
  }

  async rename(terminal) {
    const { renameDialog } = await import('./rename.js');
    if (await renameDialog(terminal.name, (name) => post(`${API}/${terminal.id}/rename`, { name })) !== null) this.refresh();
  }

  select(id, { focus = true } = {}) {
    if (id !== null && id !== this.current) {
      this.current = id;
      this.activity.delete(id);
      this.sync();
      this.draw();
    }
    if (focus) this.panes.get(this.focused)?.focus();
  }

  // The keys of the page (design 13.10).
  act(name, pane) {
    const terminal = this.terminal;
    switch (name) {
      case 'new-terminal': this.newTerminal(); break;
      case 'end-pane': this.endPane(pane); break;
      case 'previous-tab': this.select(tabAfter(this.terminals, this.current, -1)); break;
      case 'next-tab': this.select(tabAfter(this.terminals, this.current, 1)); break;
      case 'split-right': this.split('right'); break;
      case 'split-down': this.split('down'); break;
      case 'read-screen': this.readScreen(pane); break;
      default: {
        // pane-left, pane-right, pane-up and pane-down; up from a top pane is the way out, to the tab bar.
        const to = name.slice(5);
        const next = this.splits?.neighbour(this.places?.panes ?? [], terminal.panes.findIndex((one) => one.id === pane.id), to) ?? null;
        if (next !== null) this.panes.get(terminal.panes[next].id)?.focus();
        else if (to === 'up') this.dom.tabs.querySelector('[aria-selected="true"]')?.focus();
      }
    }
  }

  async readScreen(pane) {
    (await import('./read-screen.js')).readScreen(pane);
  }

  focusChanged(pane) {
    this.focused = pane.id;
    this.keys?.clear();
    this.syncKeys?.();
  }

  bell(pane) {
    if (this.motion === 'less' || matchMedia('(prefers-reduced-motion: reduce)').matches) this.mark(pane.terminal, true);
    else pane.flash();
  }

  // The dot of a tab that has output the person has not seen.
  mark(terminal, force = false) {
    if ((force || terminal !== this.current) && !this.activity.has(terminal)) {
      this.activity.add(terminal);
      this.drawTabs();
    }
  }

  async output(pane, bytes) {
    this.mark(pane.terminal);
    if (!this.prefs.announce || pane.id !== this.focused || !this.dom) return;
    const { Voice } = await import('./voice.js');
    this.voice ??= new Voice(this.dom.live);
    this.voice.region = this.dom.live;
    this.voice.push(bytes);
  }

  // ---- drawing ----

  build() {
    // A split button: New makes a terminal, and the arrow has the rest.
    const more = menu({ label: 'More terminal actions', content: ['\u25be'], items: () => this.newMenu() });
    more.button.setAttribute('aria-label', 'More terminal actions');
    const fresh = button({ class: 'topbtn tr-fresh', 'aria-label': 'New terminal', onclick: () => this.newTerminal() }, icon('plus'), 'New');
    const split = h('span', { class: 'tr-new' }, fresh, more.element);
    const tools = [
      tool({ onclick: () => this.split('right') }, 'Split right'),
      tool({ onclick: () => this.split('down') }, 'Split down'),
      tool({ 'aria-label': 'Smaller text', title: 'Smaller text', onclick: () => this.size(-1) }, 'A\u2212'),
      tool({ 'aria-label': 'Larger text', title: 'Larger text', onclick: () => this.size(1) }, 'A+'),
      this.keysButton = tool({ 'aria-pressed': 'false', onclick: () => this.toggleKeys() }, 'Keys'),
      tool({ onclick: () => (document.fullscreenElement ? document.exitFullscreen() : this.dom.root.requestFullscreen?.()) }, 'Full screen'),
    ];
    this.dom = {
      tabs: h('div', { class: 'tr-tabs', role: 'tablist', 'aria-label': 'Terminals' }),
      second: h('div', { class: 'tr-second', role: 'tablist', 'aria-label': 'Panes of this terminal', hidden: true }),
      grid: h('div', { class: 'tr-grid' }),
      keys: h('div', { class: 'tr-keys', role: 'toolbar', 'aria-label': 'Terminal keys', hidden: true }),
      status: h('span', { class: 'tr-status', role: 'status' }),
      live: h('div', { class: 'sr-only', 'aria-live': 'polite', 'aria-atomic': 'true' }),
      empty: h('div', { class: 'tr-empty', hidden: true }, h('p', {}, 'No terminal is open.'), button({ class: 'btn default', onclick: () => this.newTerminal() }, 'New terminal')),
    };
    this.dom.root = h('div', { class: 'tr-ws' },
      h('div', { class: 'tr-top' }, this.dom.tabs, split, h('div', { class: 'tr-tools' }, ...tools)),
      this.dom.second,
      h('div', { class: 'tr-main' }, this.dom.grid, this.dom.empty),
      this.dom.keys,
      h('div', { class: 'tr-foot' }, this.dom.status, this.dom.live));
    this.view.main.replaceChildren(this.dom.root);
  }

  draw() {
    if (!this.dom) return;
    this.drawTabs();
    this.drawPanes();
    this.drawStatus();
    this.drawKeys();
  }

  drawStatus() {
    if (this.dom) this.dom.status.textContent = statusWords(this.status);
  }

  adminWords() {
    const minutes = unlockMinutes();
    for (const pane of this.panes.values()) {
      if (!pane.admin) continue;
      pane.words.textContent = adminWords(minutes);
      pane.reset.hidden = !nearsEnd(minutes);
    }
  }

  // Reset to 15 min, in the head of an administrator pane that is about to end.
  async extend() {
    try {
      setUnlockSeconds((await post('/api/1/unlock/extend')).unlock_seconds);
    } catch (error) {
      this.view?.notice(error.title, { kind: 'warn' });
    }
  }

  // The key bar is shown on a phone and behind the Keys button elsewhere; its buttons are made the first time.
  async drawKeys() {
    const on = this.phone || this.keysOn === true;
    this.dom.keys.hidden = !on;
    this.keysButton.setAttribute('aria-pressed', String(this.keysOn === true));
    if (!on || !this.dom || this.dom.keys.children.length > 0) return;
    const module = await import('./keybar.js');
    this.keys ??= new module.KeyBar();
    if (!this.dom) return;
    this.syncKeys = module.fillKeys(this.dom.keys, this.keys, {
      press: (bytes) => this.panes.get(this.focused)?.send(bytes),
      modes: () => this.panes.get(this.focused)?.screen,
      refocus: () => this.panes.get(this.focused)?.focus(),
    });
    // A key of the keyboard spends a waiting Ctrl or Alt in the pane: the bar shows it.
    this.keys.changed = this.syncKeys;
  }

  toggleKeys() {
    this.keysOn = !this.keysOn;
    this.drawKeys();
    this.fit();
  }

  size(step) {
    const next = sizeStep(this.settings.size, step);
    if (next !== null) this.view.setPrefs({ terminal: { ...this.prefs, size: next } }).catch((error) => this.view.problem(error));
  }

  newMenu() {
    const terminal = this.terminal;
    const items = [];
    if (this.session.admin) items.push({ label: 'New administrator terminal', onSelect: () => this.newTerminal({ admin: true }) }, { separator: true });
    items.push({ label: 'Split right', onSelect: () => this.split('right') }, { label: 'Split down', onSelect: () => this.split('down') });
    if (terminal) {
      if (terminal.panes.length > 1 && this.splits) items.push(...this.splits.layoutItems(this, terminal));
      items.push({ separator: true }, { label: 'Rename this terminal', onSelect: () => this.rename(terminal) }, { label: 'Close this terminal', onSelect: () => this.closeTab(terminal) });
    }
    return items;
  }

  paneMenu(pane) {
    const copy = () => pane.copy().then(this.on.said);
    return [
      { label: 'Read the screen', onSelect: () => this.readScreen(pane) },
      ...(pane.selected === '' ? [] : [{ label: 'Copy', onSelect: copy }]),
      { label: 'Paste', onSelect: () => pane.paste() },
      { label: 'Clear the scrollback', onSelect: () => pane.clearScrollback() },
      { separator: true },
      { label: 'Split right', onSelect: () => this.split('right') },
      { label: 'Split down', onSelect: () => this.split('down') },
      { separator: true },
      { label: pane.ended ? 'Close this pane' : 'End this pane', onSelect: () => this.endPane(pane) },
    ];
  }

  drawTabs() {
    const { tabs } = this.dom ?? {};
    if (!tabs) return;
    const focusing = tabs.contains(document.activeElement) ? document.activeElement.dataset.id : null;
    tabs.replaceChildren(...this.terminals.map((terminal) => {
      const on = terminal.id === this.current;
      const admin = terminal.panes.some((pane) => pane.admin);
      const activity = this.activity.has(terminal.id);
      return h('div', { class: `tr-tab${on ? ' on' : ''}`, role: 'presentation' },
        button({
          role: 'tab',
          class: 'tr-tab-name',
          'data-id': terminal.id,
          'aria-selected': String(on),
          tabindex: on ? '0' : '-1',
          'aria-label': tabName({ name: terminal.name, admin, activity }),
          onclick: () => this.select(terminal.id),
          ondblclick: () => this.rename(terminal),
          onkeydown: (event) => this.tabKey(event, terminal),
        }, admin && icon('security'), terminal.name, activity && h('span', { class: 'tr-dot', 'aria-hidden': 'true' })),
        // For a pointer or a finger: a tablist holds tabs only, so a keyboard has Delete on the tab and the menu's Close this terminal.
        button({
          class: 'tr-x', title: `Close ${terminal.name}`, tabindex: '-1', 'aria-hidden': 'true', onclick: () => this.closeTab(terminal),
        }, icon('x')));
    }));
    if (focusing) tabs.querySelector(`[data-id="${focusing}"]`)?.focus();
  }

  tabKey(event, terminal) {
    const step = { ArrowRight: 1, ArrowLeft: -1 }[event.key];
    const at = { Home: this.terminals.at(0).id, End: this.terminals.at(-1).id }[event.key] ?? (step && tabAfter(this.terminals, terminal.id, step));
    if (at) {
      event.preventDefault();
      this.dom.tabs.querySelector(`[data-id="${at}"]`)?.focus();
    } else if (event.key === 'ArrowDown') {
      event.preventDefault();
      this.select(terminal.id);
    } else if (event.key === 'Delete') {
      event.preventDefault();
      this.closeTab(terminal);
    }
  }

  drawPanes() {
    const { grid, second, empty } = this.dom;
    const terminal = this.terminal;
    empty.hidden = terminal !== null;
    second.hidden = true;
    if (!terminal) return grid.replaceChildren();
    const panes = terminal.panes.map((info) => this.panes.get(info.id));
    const index = Math.max(terminal.panes.findIndex((info) => info.id === this.focused), 0);
    this.focused = terminal.panes[index]?.id ?? null;
    if (panes.length > 1 && !this.splits) {
      grid.replaceChildren();
      return this.needSplits().then(() => this.draw());
    }
    const layout = this.phone ? 'one' : terminal.layout;
    const places = panes.length > 1 && layout !== 'one'
      ? this.splits.placeOf(layout, panes.length, terminal.sizes, index)
      : { panes: panes.map((_, i) => (i === index ? WHOLE : null)), separators: [] };
    this.places = places;
    const visible = [];
    panes.forEach((pane, i) => {
      pane.label({ title: terminal.name, index: i, count: panes.length });
      const place = places.panes[i];
      if (!place) return;
      Object.assign(pane.element.style, {
        left: percent(place.x), top: percent(place.y), right: percent(1 - place.x - place.w), bottom: percent(1 - place.y - place.h),
      });
      visible.push(pane.element);
    });
    // Putting the elements back must not take focus from the one that has it.
    const active = grid.contains(document.activeElement) ? document.activeElement : null;
    const separators = places.separators.map((one) => this.splits.separator(this, terminal, one, layout));
    grid.replaceChildren(...visible, ...separators);
    active?.focus({ preventScroll: true });
    for (const pane of panes) pane.renderer.refresh();
    this.adminWords();
    second.hidden = !(this.phone && panes.length > 1);
    second.replaceChildren(...panes.map((pane, i) => button({
      role: 'tab',
      class: 'tr-tab-name',
      'aria-selected': String(i === index),
      tabindex: i === index ? '0' : '-1',
      onclick: () => {
        this.focused = pane.id;
        this.draw();
        pane.focus();
      },
    }, `Pane ${i + 1}`)));
    requestAnimationFrame(() => this.fit());
    return undefined;
  }

  // Each pane in view takes the size of its room, and the socket hears of it.
  fit() {
    if (this.dom) for (const pane of this.panes.values()) if (pane.element.isConnected) pane.fit();
  }
}
