// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The class search that decides which rules of console.css may leave it:
// for each class a rule of console.css defines, the files of the shell's
// start that name it as a word, and the other modules that put it on an
// element. A rule whose classes no start file names is only needed by the
// pages and dialogs that draw it, and can move into their sheet. Run it with
// `node userspace/admin/tests/css-reach.mjs` before moving a rule, and read
// which modules outside pages/ name a class: a module the shell loads later
// over any page needs its rules where every page has them. The functions are
// shared with css-reach.test.mjs, which holds the result for every graph.
// Not a test itself.

import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

import { graphOf, ROOT, withoutComments } from './graph.mjs';

const read = (file) => readFileSync(file, 'utf8');

/**
 * The rules of `css`, those inside an `@media` or `@container` block
 * included, each as `{selector, text, classes}`: its selector list, its text
 * as the file has it and the classes the selector list names. The blocks of
 * `@keyframes` and `@font-face` hold no rule of a class.
 */
export function rulesOf(css) {
  const text = css.replace(/\/\*[\s\S]*?\*\//g, '');
  const rules = [];
  const open = [];
  let from = 0;
  let skipping = 0;
  for (let at = 0; at < text.length; at++) {
    const mark = text[at];
    if (mark === '{') {
      const raw = text.slice(from, at);
      const prelude = raw.trim();
      open.push({ prelude, begin: from + raw.length - raw.trimStart().length });
      if (/^@(?:keyframes|font-face)\b/.test(prelude)) skipping += 1;
      from = at + 1;
    } else if (mark === '}') {
      const block = open.pop();
      if (/^@(?:keyframes|font-face)\b/.test(block.prelude)) skipping -= 1;
      else if (skipping === 0 && !block.prelude.startsWith('@')) {
        const classes = [...block.prelude.matchAll(/\.(-?[_a-zA-Z][\w-]*)/g)].map((found) => found[1]);
        rules.push({ selector: block.prelude, text: text.slice(block.begin, at + 1), classes });
      }
      from = at + 1;
    } else if (mark === ';') {
      from = at + 1;
    }
  }
  return rules;
}

/** The classes `css` defines: those its rules' selectors name. */
export const classesOf = (css) => new Set(rulesOf(css).flatMap((rule) => rule.classes));

/** The words of `source`, comments left out: a class that is one of them may be drawn by it. */
export const wordsOf = (source) => new Set(withoutComments(source).match(/[A-Za-z_][\w-]*/g) ?? []);

/**
 * The string literals in the expression that starts at `from` in `text`,
 * with the fixed parts of a template. The expression ends at a closing
 * bracket that nothing in it opened, at a semicolon, and, unless
 * `commas` says the list goes on, at a comma.
 */
function literalsFrom(text, from, commas) {
  const literals = [];
  let depth = 0;
  for (let at = from; at < text.length; at++) {
    const mark = text[at];
    if (mark === "'" || mark === '"') {
      let end = at + 1;
      while (end < text.length && text[end] !== mark) end += text[end] === '\\' ? 2 : 1;
      literals.push(text.slice(at + 1, end));
      at = end;
    } else if (mark === '`') {
      let piece = '';
      let end = at + 1;
      while (end < text.length && text[end] !== '`') {
        if (text[end] === '$' && text[end + 1] === '{') {
          let inner = 1;
          for (end += 2; end < text.length && inner > 0; end++) inner += text[end] === '{' ? 1 : text[end] === '}' ? -1 : 0;
          piece += ' ';
        } else {
          piece += text[end] === '\\' ? text[end + 1] : text[end];
          end += text[end] === '\\' ? 2 : 1;
        }
      }
      literals.push(piece);
      at = end;
    } else if ('([{'.includes(mark)) {
      depth += 1;
    } else if (')]}'.includes(mark)) {
      if (depth === 0) break;
      depth -= 1;
    } else if (mark === ';' || (mark === ',' && depth === 0 && !commas)) {
      break;
    }
  }
  return literals;
}

/**
 * The classes `source` puts on an element or looks for: the value of `class:`
 * or `className =`, the arguments of `classList.add` and the like, and the
 * selectors of `querySelector`, `closest` and `matches`. A class made at run
 * time from pieces (`ic-` and a name) is not found, and nor is one that is
 * kept in a variable first.
 */
export function classNamesOf(source) {
  const text = withoutComments(source);
  const names = new Set();
  for (const found of text.matchAll(/\b(?:class|className)\s*[:=]\s*|\bclassList\.\w+\(|\.(?:querySelector(?:All)?|closest|matches)\(/g)) {
    const list = found[0].endsWith('(');
    for (const literal of literalsFrom(text, found.index + found[0].length, list)) {
      for (const word of literal.match(/[A-Za-z_][\w-]*/g) ?? []) names.add(word);
    }
  }
  return names;
}

/** The style sheets a module links through `sheet('/name.css')`, as paths from the console's folder. */
export function sheetLinksOf(source) {
  return [...withoutComments(source).matchAll(/\bsheet\(\s*['"]\/?([^'"]+\.css)['"]\s*\)/g)].map((found) => found[1]);
}

/** Every file under `dir` that ends with `ending`, the tests' left out, as paths from the console's folder. */
export function filesUnder(dir, ending) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return entry.name === 'tests' && dir === ROOT ? [] : filesUnder(path, ending);
    return path.endsWith(ending) ? [relative(ROOT, path)] : [];
  });
}

/** The shell's start: `index.html`, the scripts and sheets it names, and what app.js imports at load. */
export function startFiles() {
  const names = [...read(`${ROOT}index.html`).matchAll(/<(?:script|link)\b[^>]*?\b(?:src|href)="\/([^"]+)"/g)].map((found) => found[1]);
  const listed = names.filter((name) => /\.(?:js|css)$/.test(name)).map((name) => `${ROOT}${name}`);
  return new Set([`${ROOT}index.html`, ...listed, ...graphOf(`${ROOT}app.js`).files]);
}

/**
 * For each class of console.css, the start files that name it as a word and
 * the other modules that put it on an element:
 * `Map(class -> {start: [...], modules: [...]})`, paths from the console's folder.
 */
export function reachOf() {
  const start = [...startFiles()].map((file) => relative(ROOT, file)).filter((file) => /\.(?:js|html)$/.test(file));
  const modules = filesUnder(ROOT, '.js').filter((file) => !start.includes(file));
  const words = new Map(start.map((file) => [file, wordsOf(read(`${ROOT}${file}`))]));
  const classes = new Map(modules.map((file) => [file, classNamesOf(read(`${ROOT}${file}`))]));
  return new Map([...classesOf(read(`${ROOT}console.css`))].map((name) => [name, {
    start: start.filter((file) => words.get(file).has(name)),
    modules: modules.filter((file) => classes.get(file).has(name)),
  }]));
}

/** Whether no start file names any class of `rule`, so that the start never draws it. */
export const unusedAtStart = (rule, reach) => rule.classes.length > 0 && rule.classes.every((name) => reach.get(name).start.length === 0);

function report() {
  const reach = reachOf();
  const rules = rulesOf(read(`${ROOT}console.css`)).filter((rule) => unusedAtStart(rule, reach));
  const bytes = (list) => list.reduce((sum, rule) => sum + Buffer.byteLength(rule.text) + 1, 0);
  const icons = rules.filter((rule) => rule.classes.every((name) => name.startsWith('ic-')));
  console.log(`${rules.length} rules of console.css have only classes that no start file names: ${bytes(rules)} bytes, ${bytes(icons)} of them icons.`);
  for (const [name, found] of reach) {
    const used = rules.filter((rule) => rule.classes.includes(name));
    if (used.length === 0 || name.startsWith('ic-')) continue;
    const shell = found.modules.filter((file) => !file.startsWith('pages/'));
    const pages = found.modules.length - shell.length;
    console.log(`.${name}  ${bytes(used)} bytes  ${[...shell, pages > 0 && `${pages} in pages/`].filter(Boolean).join(', ') || 'no module'}`);
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) report();
