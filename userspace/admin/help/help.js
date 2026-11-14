// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The help pages the console shows itself (design 9.2): /help/errors lists
// the error pages by area with a filter, /help/errors/ID shows one. The
// pages are files on this machine (the build copies docs/errors into the
// console's folder with an index.json); nothing is fetched from outside it.

import { h, uniqueId } from '../dom.js';
import { parse, render, titleOf } from '../lib/markdown.js';
import { byArea, countWords, isErrorId, matching } from './model.js';

const ERRORS = '/help/errors';

/** The index of error pages, `[{id, title, area}]` in ID order, kept for the console's life. `area` is the README's area text up to its first colon. */
let index = null;

async function readIndex(signal) {
  if (index) return index;
  const response = await fetch(`${ERRORS}/index.json`, { signal, credentials: 'same-origin' });
  if (!response.ok) throw new Error('The list of error pages is not on this server.');
  index = await response.json();
  return index;
}

/** The set of IDs that have a page, for the links in a page's words; empty when the list cannot be read. */
async function knownIds(signal) {
  try {
    return new Set((await readIndex(signal)).map((page) => page.id));
  } catch {
    return new Set();
  }
}

function notHere(view, words) {
  view.head({ title: 'No such help page' });
  view.main.append(
    h('p', {}, words),
    h('div', { class: 'btns' }, h('a', { class: 'btn', href: ERRORS }, 'What error messages mean')));
}

async function showList(view) {
  view.head({ title: 'What error messages mean', help: false });
  let pages;
  try {
    pages = await readIndex(view.signal);
  } catch (error) {
    if (view.signal.aborted) return;
    notHere(view, error.message);
    return;
  }
  const input = h('input', { id: 'filter', class: 'input', type: 'search', autocomplete: 'off' });
  const status = h('p', { class: 'small muted', role: 'status' });
  const list = h('div', { class: 'help-list' });
  const draw = () => {
    const found = matching(pages, input.value);
    status.textContent = countWords(found.length);
    // An area is a phrase ("The web console, Dukovo Admin"), so its heading's id is a counted one.
    list.replaceChildren(...byArea(found).map(({ area, pages: inArea }) => {
      const id = uniqueId('area');
      return h('section', { 'aria-labelledby': id },
        h('h2', { id }, area),
        h('ul', { class: 'help-pages' }, ...inArea.map((page) => h('li', {},
          h('a', { href: `${ERRORS}/${page.id}` }, h('span', { class: 'mono' }, page.id), ' ', page.title)))));
    }));
  };
  input.addEventListener('input', draw);
  view.main.append(
    h('p', { class: 'lead' }, 'Every error the console shows has an ID. Each ID has a page here that says what it means and what to do. The pages are on this machine.'),
    h('div', { class: 'field' }, h('label', { for: 'filter' }, 'Filter by ID or words'), input),
    status,
    list);
  draw();
}

async function showOne(view, id) {
  let response;
  try {
    response = await fetch(`${ERRORS}/${id}.md`, { signal: view.signal, credentials: 'same-origin' });
  } catch (error) {
    if (view.signal.aborted) return;
    throw error;
  }
  if (!response.ok) {
    notHere(view, `There is no help page for ${id}.`);
    return;
  }
  const blocks = parse(await response.text());
  const title = titleOf(blocks) || id;
  view.head({ title, crumbs: [{ title: 'What error messages mean', path: ERRORS }] });
  const known = await knownIds(view.signal);
  const rest = blocks.filter((block) => !('heading' in block && block.level === 1));
  view.main.append(h('article', { class: 'article' }, ...render(rest, h, known)));
}

export async function show(view) {
  const [area, id] = view.rest.split('/');
  if (view.rest === '' || (area === 'errors' && !id)) {
    await showList(view);
  } else if (area === 'errors' && isErrorId(id)) {
    await showOne(view, id);
  } else {
    notHere(view, 'The console has help pages for its error messages.');
  }
}

