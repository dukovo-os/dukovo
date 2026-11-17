// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The Show command panel the ShowCommand board draws (design 6): four tabs,
// Command, API, Ansible and Files, each a `<pre>` in the mono font with a
// Copy button; Files shows the diff of the newest change of each file in the
// journal. It is apart from lib/command.js so that the console's start does
// not carry it: it is read when a panel is first shown.

import {
  asSentence, h, icon, sheet, uniqueId,
} from '../dom.js';
import { copyText, COPIED, PRESS_CTRL_C } from './copy.js';
import {
  changeFor, diffLines, diffOf, FOOTNOTE, TABS, textOfTab,
} from './panel-model.js';

/** How long "Copied." stays. */
const STATUS_MS = 3000;

/** How many of the journal's newest rows are read to find a file's change. */
export const JOURNAL_ROWS = 200;

/** The diff of the newest change that touched `path`, as text, or the words for why there is none. */
async function diffFor(path, { get, cache }) {
  cache.journal ??= get(`/api/1/changes?limit=${JOURNAL_ROWS}`).then((answer) => answer.changes ?? []);
  const found = changeFor(await cache.journal, path);
  if (!found) return diffOf({ files: [] }, path);
  if (!cache.changes.has(found.id)) cache.changes.set(found.id, get(`/api/1/changes/${found.id}`));
  return diffOf(await cache.changes.get(found.id), path);
}

function filesTab(answer, deps, signal) {
  const box = h('div', { class: 'files' });
  if (answer.files.length === 0) {
    box.append(h('p', { class: 'small muted' }, 'This call changes no file.'));
    return { box, text: async () => '' };
  }
  const pending = answer.files.map(async (path) => {
    const part = h('div', { class: 'file' }, h('b', { class: 'mono' }, path));
    box.append(part);
    let found;
    try {
      found = await diffFor(path, deps);
    } catch {
      found = { text: null, why: 'The journal could not be read now.' };
    }
    if (signal?.aborted) return null;
    if (found.text === null) {
      part.append(h('p', { class: 'small muted' }, found.why));
    } else {
      part.append(h('pre', { class: 'cmdtext diff', tabindex: '0', 'aria-label': `Change of ${path}` },
        ...diffLines(found.text).map((line) => h('span', { class: `line ${line.kind}` },
          line.words && h('span', { class: 'sr-only' }, line.words), line.text, '\n'))));
    }
    return found.text;
  });
  return { box, text: async () => textOfTab(answer, 'files', await Promise.all(pending)) };
}

/**
 * The panel for `action` with `args`. `deps` is `{get, load, cache}`:
 * `load(action, args)` is the cached `POST /api/1/show`, and `cache` is
 * `{journal, changes}`, kept by lib/command.js until the page is left.
 */
export function panel(deps, action, args = {}, { signal } = {}) {
  const root = h('div', { class: 'cmd', 'aria-label': 'The command, the API call, the Ansible task and the files' });
  const status = h('span', { class: 'small', role: 'status' });
  let timer = null;
  const say = (words) => {
    status.textContent = words;
    clearTimeout(timer);
    timer = setTimeout(() => status.replaceChildren(), STATUS_MS);
  };
  root.append(h('p', { class: 'small muted' }, 'Loading the command…'));

  // The head of the panel has its look from dialogs.css, which the panel waits for.
  Promise.all([deps.load(action, args), sheet('/dialogs.css')]).then(([answer]) => {
    const id = uniqueId('cmd');
    const files = filesTab(answer, deps, signal);
    const bodies = {
      command: h('pre', { class: 'cmdtext', tabindex: '0' }, answer.command),
      api: h('pre', { class: 'cmdtext', tabindex: '0' }, answer.api),
      ansible: h('pre', { class: 'cmdtext', tabindex: '0' }, answer.ansible),
      files: files.box,
    };
    const tabs = TABS.map((tab) => h('button', {
      class: 'tab', type: 'button', role: 'tab', id: `${id}-${tab.id}`, 'aria-controls': `${id}-panel-${tab.id}`,
    }, tab.label));
    const panels = TABS.map((tab) => h('div', {
      role: 'tabpanel', id: `${id}-panel-${tab.id}`, 'aria-labelledby': `${id}-${tab.id}`, hidden: true,
    }, bodies[tab.id]));
    let current = 0;
    const select = (at, focus = false) => {
      current = (at + TABS.length) % TABS.length;
      tabs.forEach((tab, index) => {
        tab.setAttribute('aria-selected', String(index === current));
        tab.tabIndex = index === current ? 0 : -1;
        panels[index].hidden = index !== current;
      });
      if (focus) tabs[current].focus();
      copyAnsible.hidden = TABS[current].id !== 'command';
    };
    const copy = async (tab) => {
      const text = tab === 'files' ? await files.text() : textOfTab(answer, tab);
      say((await copyText(text, bodies[tab])) ? COPIED : PRESS_CTRL_C);
    };
    const copyButton = h('button', { class: 'btn small', type: 'button', onclick: () => copy(TABS[current].id) }, icon('copy'), 'Copy');
    const copyAnsible = h('button', { class: 'btn small', type: 'button', onclick: () => copy('ansible') }, 'Copy as Ansible task');
    const list = h('div', { class: 'tabs', role: 'tablist', 'aria-label': 'Show as' }, ...tabs);
    list.addEventListener('keydown', (event) => {
      const to = { ArrowRight: current + 1, ArrowLeft: current - 1, Home: 0, End: TABS.length - 1 }[event.key];
      if (to === undefined) return;
      event.preventDefault();
      select(to, true);
    });
    tabs.forEach((tab, index) => tab.addEventListener('click', () => select(index)));
    root.replaceChildren(
      h('div', { class: 'cmd-head' }, icon('code'), list, h('div', { class: 'btns' }, copyButton, copyAnsible)),
      ...panels,
      status,
      h('p', { class: 'small muted' }, FOOTNOTE));
    select(0);
  }, (error) => {
    root.replaceChildren(h('p', { class: 'msg' }, icon('warn'), h('span', {}, asSentence(error.title), ' ', error.detail ?? '')));
  });
  return root;
}
