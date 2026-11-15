// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The help pages' lists as plain values (design 9.2): the error pages by
// area, filtered by what a person types.

/** The pages of `index` (`[{id, title, area}]`) matching `filter`, in the order of their IDs. */
export function matching(index, filter) {
  const words = filter.toLowerCase().split(/\s+/).filter(Boolean);
  return index
    .filter((page) => words.every((word) => `${page.id} ${page.title} ${page.area ?? ''}`.toLowerCase().includes(word)))
    .sort((one, other) => (one.id < other.id ? -1 : one.id > other.id ? 1 : 0));
}

/** The area a page is listed under: its `area`, or the letters of its ID. */
export const areaOf = (page) => page.area || page.id.split('-')[0];

/** `pages` grouped by area, the areas in the order of their first page (the pages come in ID order, which is the order of the README's table): `[{area, pages}]`. */
export function byArea(pages) {
  const groups = new Map();
  for (const page of pages) {
    const area = areaOf(page);
    if (!groups.has(area)) groups.set(area, []);
    groups.get(area).push(page);
  }
  return [...groups.entries()].map(([area, list]) => ({ area, pages: list }));
}

/** An error ID as the URL holds it: letters, a hyphen and three digits. */
export const isErrorId = (text) => /^[A-Z]{2,5}-\d{3}$/.test(text);

/** The words that say how many pages a filter left. */
export const countWords = (count) => (count === 0 ? 'No page matches.' : `${count} ${count === 1 ? 'page' : 'pages'}`);
