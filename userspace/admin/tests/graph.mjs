// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Walks the console's import graph from files on disk, for the size budget
// (design 3.6). Not a test itself. A static `import` or `export ... from`
// is loaded with the file that has it; a `import('...')` is another start
// and has a graph of its own.

import { readFileSync, statSync } from 'node:fs';
import { dirname, relative, resolve } from 'node:path';

export const ROOT = new URL('../', import.meta.url).pathname;

/** `source` without block comments and without the lines that are only a comment. */
function withoutComments(source) {
  return source.replace(/\/\*[\s\S]*?\*\//g, '').split('\n').filter((line) => !line.trim().startsWith('//')).join('\n');
}

/** The specifiers `source` imports `{fixed, later}`: at load, and with `import()`. */
export function importsOf(source) {
  const text = withoutComments(source);
  const fixed = [...text.matchAll(/^\s*(?:import|export)\s[^'"`;]*?from\s*['"]([^'"]+)['"]/gm)].map((found) => found[1]);
  fixed.push(...[...text.matchAll(/^\s*import\s*['"]([^'"]+)['"]/gm)].map((found) => found[1]));
  const later = [...text.matchAll(/\bimport\(\s*['"]([^'"]+)['"]\s*\)/g)].map((found) => found[1]);
  return { fixed, later };
}

/**
 * The files loaded with `start` (an absolute path): itself and every file
 * it imports at load, and the roots of the graphs it starts later.
 * Gives `{files: Set, later: Set}`; a later root that does not exist is
 * left out, since a part may be absent (design 3.5).
 */
export function graphOf(start, known = new Set()) {
  const files = new Set();
  const later = new Set();
  const visit = (file) => {
    if (files.has(file) || known.has(file)) return;
    files.add(file);
    const { fixed, later: dynamic } = importsOf(readFileSync(file, 'utf8'));
    for (const specifier of fixed) visit(resolve(dirname(file), specifier));
    for (const specifier of dynamic) {
      const target = resolve(dirname(file), specifier);
      try {
        statSync(target);
        later.add(target);
      } catch {
        // A part that is not there yet.
      }
    }
  };
  visit(start);
  return { files, later };
}

/** The size in bytes of the files of `set`, as served. */
export const sizeOf = (set) => [...set].reduce((sum, file) => sum + statSync(file).size, 0);

/** `file` relative to the console's folder, for messages. */
export const shown = (file) => relative(ROOT, file);
