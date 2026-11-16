// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The command palette (design 11), which Ctrl+K and the header's Find
// button open: a modal <dialog> with a search field and a list of what it
// finds (pages, actions, services, certificates, terminal sessions), as the
// Palette board draws it. Enter opens a page, runs a read and shows its
// answer, or opens the run view of a change; Tab opens the run view with the
// Show command panel under the form. Esc goes back from those views to the
// list, and closes the palette from the list.
//
// The field is an ARIA combobox: it keeps focus, and `aria-activedescendant`
// names the row in hand. index-model.js decides what the rows are and
// lib/fuzzy.js what each query finds; this file draws them and takes keys.

import { focusBack, h, icon, sheet, uniqueId } from '../dom.js';
import { changeDone, findAction, taskStarted } from '../lib/action-model.js';
import { showKey } from '../lib/command-model.js';
import { highlight } from '../lib/fuzzy.js';
import { keep, recall } from '../lib/store.js';
import { PAGES } from '../pages.js';
import { isUnlocked, onUnlockChange } from '../unlock.js';
import {
  applyRecent, argsReady, buildIndex, cleanRecent, countWords, find, flatten, groupResults, pillsFor, recentName, remember, runWords,
} from './index-model.js';
import { resultView } from './result-view.js';
import { runView, viewHead } from './run-view.js';

// How long the things the parts gave are kept, and how long a person stops
// typing before the number of results is said, and before the command of the
// row in hand is read.
const THINGS_MS = 30_000;
const SAY_MS = 300;
const COMMAND_MS = 150;

// What was read once, for as long as this page is open. `shown` is the last
// things read, which the next opening shows at once while it reads them
// again, so that a recent row is in its place from the first frame.
let actionsRead = null;
let things = null;
let shown = null;
let current = null;

/** The list of actions: the shell's own, when it gives one, or one read here. */
function loadActions(cx) {
  if (cx.actions) return cx.actions();
  actionsRead ??= cx.get('/api/1/actions').catch((error) => {
    actionsRead = null;
    throw error;
  });
  return actionsRead;
}

/** What a page's palette part gives, read as one value: its phrases, and each provider with its items. */
async function collectPart(module, pcx) {
  return {
    phrases: module.phrases ?? {},
    groups: await Promise.all((module.providers ?? []).map(async (provider) => ({
      group: provider.group, kind: provider.kind, filters: provider.filters ?? {}, items: await provider.items(pcx),
    }))),
  };
}

/** The things of every page's palette part, as `parts.collect` gives them, kept for 30 s. */
function loadThings(cx, name) {
  if (things && things.name === name && Date.now() - things.at < THINGS_MS) return things.promise;
  const promise = cx.parts.collect('palette', collectPart);
  things = { name, at: Date.now(), promise };
  promise.then((found) => {
    shown = { name, found };
  }, () => {
    if (things?.promise === promise) things = null;
  });
  return promise;
}

let announcer = null;

/** Says `words` to a screen reader once the palette has closed, since a modal dialog makes the rest of the page inert while it is open. */
function announce(words) {
  if (!announcer?.isConnected) {
    announcer = h('div', { class: 'sr-only', role: 'status', 'aria-live': 'polite' });
    document.body.append(announcer);
  }
  announcer.textContent = '';
  setTimeout(() => {
    announcer.textContent = words;
  }, 100);
  setTimeout(() => announcer?.replaceChildren(), 8000);
}

/** The keys under the list, from the board. */
function footer(hints) {
  const key = (text) => h('span', { class: 'kbd' }, text);
  return h('div', { class: 'pal-foot' },
    h('span', { class: 'keys' }, key('↑'), ' ', key('↓'), ' move'),
    h('span', { class: 'keys' }, key('Enter'), ' open or run'),
    h('span', { class: 'keys' }, key('Tab'), ' show the command first'),
    h('span', { class: 'keys' }, key('Ctrl K'), ' close'),
    h('span', {}, ...hints.flatMap((hint) => [key(hint), ' ']), 'filter by kind'));
}

/** The palette for `cx`; gives `{close}`. `ended()` is told when it has closed. */
function createPalette(cx, ended) {
  const session = cx.session();
  const listId = uniqueId('pal-list');
  let recent = cleanRecent(recall(recentName(session.name)));
  let actions = null;
  let collected = shown?.name === session.name ? shown.found : [];
  let index = buildIndex({ pages: PAGES, actions: [], collected, session });
  let query = '';
  let groups = [];
  let flat = [];
  let active = 0;
  let activeKey = null;
  let moved = false;
  let mode = 'list';
  let sub = null;
  let viewToken = 0;
  let said = null;
  let commandTimer = null;
  let cancelled = false;
  let actionsLost = false;
  const commandLines = new Map();

  const input = h('input', {
    type: 'text',
    role: 'combobox',
    'aria-label': 'Search pages, services, settings and actions',
    'aria-expanded': 'false',
    'aria-controls': listId,
    'aria-autocomplete': 'list',
    autocomplete: 'off',
    autocapitalize: 'off',
    spellcheck: 'false',
    enterkeyhint: 'go',
    placeholder: 'Search or run a command',
  });
  const searchRow = h('div', { class: 'pal-in' }, icon('search'), input, h('span', { class: 'kbd', 'aria-hidden': 'true' }, 'Esc'));
  const list = h('div', { id: listId, class: 'pal-list', role: 'listbox', 'aria-label': 'Results' });
  const empty = h('p', { class: 'pal-empty', hidden: true });
  const lost = h('p', { class: 'pal-note', hidden: true }, 'The list of actions could not be read, so only pages and things are here.');
  const status = h('div', { class: 'sr-only', role: 'status', 'aria-live': 'polite' });
  let foot = footer(index.hints);
  const pane = h('div', { class: 'pal-view', hidden: true });
  const dialog = h('dialog', { class: 'pal', 'aria-label': 'Search or run a command' }, searchRow, lost, list, empty, foot, pane, status);

  const say = (words) => {
    status.textContent = '';
    status.textContent = words;
  };
  const sayLater = (words) => {
    clearTimeout(said);
    said = setTimeout(() => say(words), SAY_MS);
  };

  const who = () => ({ admin: session.admin === true, unlocked: isUnlocked() });

  // The command of the row in hand, one line under its title, when its arguments are all there.
  function showCommand() {
    clearTimeout(commandTimer);
    const result = flat[active];
    const target = result?.row.target;
    const entry = target?.action ? findAction(actions ?? [], target.action) : null;
    if (!entry || !argsReady(entry, target.args)) return;
    const key = showKey(entry.id, target.args);
    commandTimer = setTimeout(async () => {
      try {
        if (!commandLines.has(key)) commandLines.set(key, cx.post('/api/1/show', { action: entry.id, args: target.args }).then((answer) => answer.command));
        const command = await commandLines.get(key);
        const element = list.querySelector(`[data-key="${CSS.escape(result.row.key)}"] .pal-text`);
        if (element && flat[active] === result && !element.querySelector('.cmd-line')) element.append(h('small', { class: 'cmd-line mono' }, command));
      } catch {
        commandLines.delete(key);
      }
    }, COMMAND_MS);
  }

  function setActive(to, scroll = true, byPerson = false) {
    if (byPerson) moved = true;
    if (flat.length === 0) {
      active = 0;
      activeKey = null;
      input.removeAttribute('aria-activedescendant');
      return;
    }
    active = (to + flat.length) % flat.length;
    activeKey = flat[active].row.key;
    list.querySelectorAll('[role="option"]').forEach((option, at) => {
      const on = at === active;
      option.classList.toggle('on', on);
      option.setAttribute('aria-selected', String(on));
      if (!on) option.querySelector('.cmd-line')?.remove();
      if (on) {
        input.setAttribute('aria-activedescendant', option.id);
        if (scroll) option.scrollIntoView({ block: 'nearest' });
      }
    });
    showCommand();
  }

  function optionFor(result, at) {
    const { row, marks } = result;
    const pills = pillsFor(row, who());
    const title = highlight(row.title, marks).map((run) => (run.hit ? h('mark', { class: 'mark-hit' }, run.text) : run.text));
    // The option is named by its words alone, so that a screen reader does not say the marks.
    const name = [row.title, row.detail, row.state?.label, ...pills.map((pill) => pill.label)].filter(Boolean).join(', ');
    const option = h('div', {
      id: `${listId}-${at}`, class: 'pal-item', role: 'option', 'aria-selected': 'false', 'aria-label': name, 'data-key': row.key,
    },
    icon(row.icon),
    h('span', { class: 'pal-text' }, h('span', {}, title, row.detail && h('small', {}, ' · ', row.detail))),
    h('span', { class: 'right' },
      row.state && h('span', { class: `st ${row.state.tone}` }, row.state.label),
      ...pills.map((pill) => h('span', { class: `pill ${pill.tone}`.trim() }, pill.label)),
      h('span', { class: 'kbd', 'aria-hidden': 'true' }, 'Enter')));
    option.addEventListener('mousedown', (event) => event.preventDefault());
    option.addEventListener('mousemove', () => {
      if (active !== at) setActive(at, false, true);
    });
    option.addEventListener('click', () => {
      setActive(at, false, true);
      activate(flat[at], false);
    });
    return option;
  }

  function render() {
    const results = find(index, query);
    groups = groupResults(index, results, query);
    flat = flatten(groups);
    let at = 0;
    list.replaceChildren(...groups.map((group) => h('div', { class: 'pal-group', role: 'group', 'aria-label': group.label },
      h('b', { 'aria-hidden': 'true' }, group.label),
      ...group.results.map((result) => optionFor(result, (at += 1) - 1)))));
    // The first row is in hand, unless the person has moved to another and the list only changed under them.
    const found = moved ? Math.max(0, flat.findIndex((result) => result.row.key === activeKey)) : 0;
    input.setAttribute('aria-expanded', String(flat.length > 0));
    // A listbox with no option in it is left out, for the screen reader and the checker alike.
    list.hidden = flat.length === 0;
    empty.hidden = flat.length > 0;
    if (flat.length === 0) {
      empty.textContent = query.trim() === ''
        ? 'There is nothing to find yet.'
        : `Nothing matches “${query.trim()}”. Try fewer letters, or a filter such as ${index.hints.at(-1) ?? 'kind:page'}.`;
    }
    setActive(found, false);
  }

  function rebuild() {
    index = buildIndex({ pages: PAGES, actions: actions ?? [], collected, session });
    applyRecent(index.rows, recent);
    const hints = footer(index.hints);
    foot.replaceWith(hints);
    foot = hints;
    foot.hidden = mode !== 'list';
    if (mode === 'list') render();
  }

  function remembered(row) {
    recent = remember(recent, row.key);
    keep(recentName(session.name), recent);
    applyRecent(index.rows, recent);
  }

  // The views that replace the list.
  function showSub(parts, focus) {
    mode = parts.mode;
    sub?.dispose?.();
    sub = parts.view ?? null;
    searchRow.hidden = true;
    lost.hidden = true;
    list.hidden = true;
    empty.hidden = true;
    foot.hidden = true;
    pane.hidden = false;
    pane.replaceChildren(...parts.element);
    focus();
  }

  function back() {
    viewToken += 1;
    sub?.dispose?.();
    sub = null;
    mode = 'list';
    pane.hidden = true;
    pane.replaceChildren();
    searchRow.hidden = false;
    lost.hidden = !actionsLost;
    foot.hidden = false;
    render();
    input.focus();
  }

  function showRun(row, entry, panel) {
    const view = runView(row, entry, {
      session: cx.session,
      post: cx.post,
      run: cx.run,
      commands: cx.commands,
      pills: pillsFor(row, who()),
      panel,
      onBack: back,
      onDone: (outcome) => finished(row, entry, outcome),
    });
    showSub({ mode: 'run', element: view.element, view }, () => view.focus());
  }

  function finished(row, entry, outcome) {
    remembered(row);
    things = null;
    if (!entry.changes) {
      showAnswer(row, outcome.result);
      return;
    }
    const words = runWords(row.title);
    const done = outcome.task === undefined ? (outcome.result?.done ? changeDone(words, outcome.result) : words.done) : taskStarted(words, outcome.task);
    close();
    announce(done);
  }

  function showAnswer(row, answer) {
    const head = viewHead({ title: row.title, onBack: back });
    showSub({ mode: 'result', element: [head.element, h('div', { class: 'pal-pane' }, resultView(answer))] }, () => head.heading.focus());
    say('The answer is shown.');
  }

  async function runRead(row, entry) {
    const mine = (viewToken += 1);
    const head = viewHead({ title: row.title, onBack: back });
    const slot = h('div', { class: 'pal-slot' });
    const body = h('div', { class: 'pal-pane' }, h('p', { class: 'small muted', role: 'status' }, 'Reading the answer…'), slot);
    showSub({ mode: 'result', element: [head.element, body] }, () => head.heading.focus());
    const result = await cx.run(entry.id, row.target.args, { words: runWords(row.title), where: { slot } });
    if (mine !== viewToken) return;
    if (result.ok) {
      remembered(row);
      showAnswer(row, result.result);
    } else {
      body.replaceChildren(slot);
    }
  }

  function activate(result, tab) {
    if (!result || mode !== 'list') return;
    const { row } = result;
    const { target } = row;
    if (!target) {
      say(`${row.title} has no page to open yet.`);
      return;
    }
    if (target.path) {
      remembered(row);
      // The page takes focus itself, so the palette does not give it back to the opener.
      close(false);
      cx.go(target.path);
      return;
    }
    const entry = findAction(actions ?? [], target.action);
    if (entry && !tab && !entry.changes && argsReady(entry, target.args)) runRead(row, entry);
    else showRun(row, entry, tab);
  }

  function atEdge(key) {
    return input.value === '' || (key === 'Home' ? input.selectionStart === 0 && input.selectionEnd === 0 : input.selectionStart === input.value.length);
  }

  input.addEventListener('input', () => {
    query = input.value;
    moved = false;
    render();
    sayLater(countWords(flat.length));
  });
  input.addEventListener('keydown', (event) => {
    if (event.isComposing) return;
    const keys = {
      ArrowDown: () => setActive(active + 1, true, true),
      ArrowUp: () => setActive(active - 1, true, true),
      Enter: () => activate(flat[active], false),
    };
    if (event.key === 'Tab' && !event.shiftKey && flat[active]?.row.target?.action) {
      event.preventDefault();
      activate(flat[active], true);
    } else if ((event.key === 'Home' || event.key === 'End') && !event.shiftKey && atEdge(event.key)) {
      event.preventDefault();
      setActive(event.key === 'Home' ? 0 : flat.length - 1, true, true);
    } else if (keys[event.key]) {
      event.preventDefault();
      keys[event.key]();
    }
  });

  let restore = true;
  function close(giveFocusBack = true) {
    restore = giveFocusBack;
    if (!dialog.open) {
      cancelled = true;
      return;
    }
    dialog.close();
  }

  // Esc goes back from a view to the list, and closes from the list.
  dialog.addEventListener('cancel', (event) => {
    if (mode !== 'list') {
      event.preventDefault();
      back();
    }
  });
  dialog.addEventListener('click', (event) => {
    if (event.target !== dialog) return;
    const box = dialog.getBoundingClientRect();
    if (event.clientX < box.left || event.clientX > box.right || event.clientY < box.top || event.clientY > box.bottom) close();
  });
  const opener = document.activeElement;
  const stopFollowing = onUnlockChange(() => {
    if (mode === 'list') render();
  });
  dialog.addEventListener('close', () => {
    clearTimeout(said);
    clearTimeout(commandTimer);
    stopFollowing();
    sub?.dispose?.();
    dialog.remove();
    if (restore) focusBack(opener);
    ended();
  });

  // What is read: the actions, once, and the things, kept for 30 s. The list shows the pages and the things last read at once, and grows or changes as these arrive.
  loadActions(cx).then((list_) => {
    actions = Array.isArray(list_) ? list_ : [];
    rebuild();
  }, () => {
    actions = [];
    actionsLost = true;
    lost.hidden = mode !== 'list';
    say('The list of actions could not be read. Pages and things are still here.');
  });
  loadThings(cx, session.name).then((found) => {
    collected = found;
    rebuild();
  }, () => {});
  applyRecent(index.rows, recent);

  // The dialog does not open before its sheet has loaded, so that it is never bare.
  sheet('/palette/palette.css').then(() => {
    if (cancelled) {
      ended();
      return;
    }
    document.body.append(dialog);
    dialog.showModal();
    render();
    input.focus();
  });
  return { close };
}

/**
 * Opens the palette, or closes it when it is open (Ctrl K again). `cx` is
 * the shell's: `get` and `post` (api.js's), `session()`, `time()`, `parts`,
 * `go(path)`, `run` (lib/action.js's, for the page's signal), `commands`
 * (Show command) and, when it has one, `actions()`, the list of
 * `GET /api/1/actions` that the shell keeps.
 */
export function togglePalette(cx) {
  if (current) {
    current.close();
    return;
  }
  current = createPalette(cx, () => {
    current = null;
  });
}
