// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// lib/markdown.js: the subset the error pages use, as blocks and as nodes;
// and that HTML in a page stays text. The nodes are made by a stand-in for
// dom.js's `h`, so no browser is needed.

import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import test from 'node:test';

import { inline, parse, render, titleOf } from '../lib/markdown.js';

/** An element as plain data: its tag, its attributes and its children. */
const make = (tag, attributes, ...children) => ({ tag, attributes, children });

/** The words of nodes, as a person would read them. */
const words = (nodes) => nodes.map((node) => (typeof node === 'string' ? node : words(node.children))).join('');

const PAGE = `<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# ADM-106: The console is locked

A change was asked while the console was locked. The header says *Limited
access*. It locks again after **15 minutes**.

## What to do

1. Press *Unlock* in the header.
2. Make the change again. Run \`service <name> explain\`
   to see why.

- A bullet.
- Another, see ADM-107.

## See also

authd(8), the handbook's chapter, ADM-123.
`;

test('a page is headings, paragraphs and lists, without the comment of licence words', () => {
  const blocks = parse(PAGE);
  assert.deepEqual(blocks.map((block) => Object.keys(block)[0]), [
    'heading', 'paragraph', 'heading', 'list', 'list', 'heading', 'paragraph',
  ]);
  assert.equal(titleOf(blocks), 'ADM-106: The console is locked');
  assert.deepEqual(blocks[0], { heading: 'ADM-106: The console is locked', level: 1 });
  assert.equal(blocks[1].paragraph, 'A change was asked while the console was locked. The header says *Limited access*. It locks again after **15 minutes**.');
  assert.deepEqual(blocks[3], {
    list: true,
    ordered: true,
    items: ['Press *Unlock* in the header.', 'Make the change again. Run `service <name> explain` to see why.'],
  });
  assert.deepEqual(blocks[4], { list: true, ordered: false, items: ['A bullet.', 'Another, see ADM-107.'] });
});

test('a page with no comment, or with Windows line ends, reads the same', () => {
  const plain = parse('# A: b\r\n\r\nWords\r\nmore.\r\n');
  assert.deepEqual(plain, [{ heading: 'A: b', level: 1 }, { paragraph: 'Words more.' }]);
  assert.deepEqual(parse(''), []);
  assert.deepEqual(parse('\n\n  \n'), []);
  assert.equal(titleOf([{ paragraph: 'no heading' }]), '');
});

test('a fenced block is code as it is written, and a list ends where the text goes on', () => {
  const blocks = parse('Text:\n\n```markdown\n# Not a heading\n- not a list\n```\n\n- one\n\nA paragraph.\n');
  assert.deepEqual(blocks, [
    { paragraph: 'Text:' },
    { code: '# Not a heading\n- not a list' },
    { list: true, ordered: false, items: ['one'] },
    { paragraph: 'A paragraph.' },
  ]);
  const open = parse('```\nnever closed\nstill code');
  assert.deepEqual(open, [{ code: 'never closed\nstill code' }]);
});

test('a bulleted list and a numbered list that follow each other are two lists, and a blank line between items keeps one', () => {
  const blocks = parse('- a\n1. b\n\n- c\n\n- d\n');
  assert.deepEqual(blocks, [
    { list: true, ordered: false, items: ['a'] },
    { list: true, ordered: true, items: ['b'] },
    { list: true, ordered: false, items: ['c', 'd'] },
  ]);
});

test('the inline parts are code, strong, emphasis and links, and the rest is text', () => {
  assert.deepEqual(inline('a `x < y` b'), [{ text: 'a ' }, { code: 'x < y' }, { text: ' b' }]);
  assert.deepEqual(inline('**bold** and *soft* and _also_'), [
    { strong: 'bold' }, { text: ' and ' }, { em: 'soft' }, { text: ' and ' }, { em: 'also' },
  ]);
  assert.deepEqual(inline('see [ADM-107](ADM-107.md) or [the ADR](../adr/0420-docs.md)'), [
    { text: 'see ' }, { link: 'ADM-107', to: 'ADM-107' }, { text: ' or ' }, { link: 'the ADR', to: null },
  ]);
  assert.deepEqual(inline('a * b * c and snake_case_name'), [{ text: 'a * b * c and snake_case_name' }]);
  assert.deepEqual(inline('unclosed `code and **bold'), [{ text: 'unclosed `code and **bold' }]);
});

test('an error ID in the words, and a link to an error page, are links when that page is on the machine', () => {
  const known = new Set(['ADM-107', 'ADM-123']);
  const nodes = render(parse('# T\n\nSee ADM-107, ADM-999 and [this](ADM-123.md), [that](ADM-998.md).\n'), make, known);
  const paragraph = nodes[1];
  assert.equal(words(nodes).replace('T', ''), 'See ADM-107, ADM-999 and this, that.');
  const links = paragraph.children.filter((node) => node.tag === 'a');
  assert.deepEqual(links.map((link) => [link.attributes.href, words([link])]), [
    ['/help/errors/ADM-107', 'ADM-107'],
    ['/help/errors/ADM-123', 'this'],
  ]);
});

test('every block becomes the element for it, and code goes in a block that a keyboard can scroll', () => {
  const nodes = render(parse('# A\n\n## B\n\nPara `c`.\n\n1. one\n\n```\nx\n```\n\n- up\n'), make);
  assert.deepEqual(nodes.map((node) => node.tag), ['h1', 'h2', 'p', 'ol', 'pre', 'ul']);
  assert.equal(nodes[4].attributes.tabindex, '0');
  assert.deepEqual(nodes[3].children.map((item) => item.tag), ['li']);
  assert.equal(nodes[2].children[1].tag, 'code');
});

test('HTML in a page stays text: it is never a tag, an attribute or a link', () => {
  const nodes = render(parse('# <script>alert(1)</script>\n\nHello <b onclick="x()">there</b> <img src=x onerror=y> [click](javascript:alert(1))\n\n- <a href="https://example.org">go</a>\n'), make);
  const found = [];
  (function walk(list) {
    for (const node of list) {
      if (typeof node === 'string') continue;
      found.push(node.tag);
      walk(node.children);
    }
  })(nodes);
  assert.deepEqual(found, ['h1', 'p', 'ul', 'li']);
  assert.equal(words([nodes[0]]), '<script>alert(1)</script>');
  assert.equal(words([nodes[1]]), 'Hello <b onclick="x()">there</b> <img src=x onerror=y> click)');
  assert.equal(words([nodes[2]]), '<a href="https://example.org">go</a>');
});

test('every page of docs/errors parses, with a title that names its ID', () => {
  const folder = new URL('../../../docs/errors/', import.meta.url);
  for (const file of readdirSync(folder).filter((name) => /^[A-Z]+-\d{3}\.md$/.test(name))) {
    const blocks = parse(readFileSync(new URL(file, folder), 'utf8'));
    assert.match(titleOf(blocks), new RegExp(`^${file.replace('.md', '')}: `), file);
    assert.ok(blocks.length > 2, file);
    for (const block of blocks.filter((each) => 'heading' in each)) {
      assert.ok(block.level === 1 || block.level === 2, `${file}: a heading of level ${block.level}`);
    }
  }
});
