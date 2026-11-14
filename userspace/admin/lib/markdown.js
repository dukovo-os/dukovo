// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The subset of Markdown the error pages in docs/errors use, as a list of
// blocks and as DOM nodes: headings, paragraphs, bulleted and numbered
// lists, fenced code, and inside the text `code`, **strong**, *emphasis*
// and links to other error pages. Anything else shows as its text, and
// text is only ever text: a `<b>` in a page is the characters `<b>` on the
// screen, since the nodes are made with text and never with markup.

// An error ID: the letters of an area, a hyphen and three digits.
const ID = /\b([A-Z]{2,5}-\d{3})\b/;

const HEADING = /^(#{1,6})\s+(.*?)\s*#*\s*$/;
const ITEM = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/;
const FENCE = /^\s*```\s*([\w-]*)\s*$/;

/** Whether `line` starts a block that ends a paragraph. */
const startsBlock = (line) => HEADING.test(line) || ITEM.test(line) || FENCE.test(line);

/** `lines` without the comment of licence words that every page starts with. */
function withoutComment(lines) {
  let first = 0;
  while (first < lines.length && lines[first].trim() === '') first += 1;
  if (!lines[first]?.trimStart().startsWith('<!--')) return lines;
  let last = first;
  while (last < lines.length && !lines[last].includes('-->')) last += 1;
  return lines.slice(last + 1);
}

/**
 * The inline parts of `text`: `{text}`, `{code}`, `{strong}`, `{em}` and
 * `{link, to}`, where `to` is the error page a link names or null for a
 * link to anywhere else, which shows as its words alone.
 */
export function inline(text) {
  const parts = [];
  let plain = '';
  const flush = () => {
    if (plain !== '') parts.push({ text: plain });
    plain = '';
  };
  let at = 0;
  while (at < text.length) {
    const rest = text.slice(at);
    let found;
    if (rest.startsWith('`') && (found = /^(`+)([\s\S]*?[^`])\1(?!`)/.exec(rest))) {
      flush();
      parts.push({ code: found[2].trim() });
    } else if ((found = /^\*\*([^*]+?)\*\*/.exec(rest))) {
      flush();
      parts.push({ strong: found[1] });
    } else if ((found = /^(\*|_)([^*_\s][^*_]*?)\1(?![\w*])/.exec(rest)) && (at === 0 || !/\w/.test(text[at - 1]))) {
      flush();
      parts.push({ em: found[2] });
    } else if ((found = /^\[([^\]]+)\]\(([^)\s]*)\)/.exec(rest))) {
      flush();
      const named = /^(?:.*\/)?([A-Z]{2,5}-\d{3})(?:\.md)?$/.exec(found[2]);
      parts.push({ link: found[1], to: named ? named[1] : null });
    } else {
      plain += text[at];
      at += 1;
      continue;
    }
    at += found[0].length;
  }
  flush();
  return parts;
}

/**
 * The blocks of `source`: `{heading, level}`, `{paragraph}`, `{list,
 * ordered}`, `{code}`. A heading, paragraph or item holds its text for
 * `inline`; a list holds `items`, a code block its `text`.
 */
export function parse(source) {
  const lines = withoutComment(source.replace(/\r\n?/g, '\n').split('\n'));
  const blocks = [];
  let at = 0;
  while (at < lines.length) {
    const line = lines[at];
    if (line.trim() === '') {
      at += 1;
    } else if (FENCE.test(line)) {
      const body = [];
      at += 1;
      while (at < lines.length && !FENCE.test(lines[at])) {
        body.push(lines[at]);
        at += 1;
      }
      at += 1;
      blocks.push({ code: body.join('\n') });
    } else if (HEADING.test(line)) {
      const [, marks, words] = HEADING.exec(line);
      blocks.push({ heading: words, level: marks.length });
      at += 1;
    } else if (ITEM.test(line)) {
      const ordered = /\d/.test(ITEM.exec(line)[2]);
      const items = [];
      while (at < lines.length) {
        const item = ITEM.exec(lines[at]);
        if (!item || /\d/.test(item[2]) !== ordered) break;
        let words = item[3];
        at += 1;
        // A line that is no new item, heading or fence, and that is not
        // blank, goes on with the item, as it does in a paragraph.
        while (at < lines.length && lines[at].trim() !== '' && !startsBlock(lines[at])) {
          words += ` ${lines[at].trim()}`;
          at += 1;
        }
        items.push(words);
        // A blank line between items keeps the list when an item follows.
        let next = at;
        while (next < lines.length && lines[next].trim() === '') next += 1;
        if (next > at && next < lines.length && ITEM.test(lines[next])) at = next;
      }
      blocks.push({ list: true, ordered, items });
    } else {
      let words = line.trim();
      at += 1;
      while (at < lines.length && lines[at].trim() !== '' && !startsBlock(lines[at])) {
        words += ` ${lines[at].trim()}`;
        at += 1;
      }
      blocks.push({ paragraph: words });
    }
  }
  return blocks;
}

/** The words of the first heading of `blocks`, or an empty string. */
export function titleOf(blocks) {
  return blocks.find((block) => 'heading' in block)?.heading ?? '';
}

/**
 * The nodes for `text`, made with `make(tag, attributes, ...children)`
 * (dom.js's `h`). An error ID in the words, and a link to an error page,
 * is a link to `/help/errors/ID` when `known` has that ID.
 */
function inlineNodes(text, make, known) {
  const toPage = (id, words) => make('a', { href: `/help/errors/${id}` }, words);
  return inline(text).flatMap((part) => {
    if ('code' in part) return make('code', {}, part.code);
    if ('strong' in part) return make('strong', {}, part.strong);
    if ('em' in part) return make('em', {}, part.em);
    if ('link' in part) return part.to && known.has(part.to) ? toPage(part.to, part.link) : part.link;
    // Each ID in the words is a link, and the words between stay words.
    const nodes = [];
    let rest = part.text;
    for (let found = ID.exec(rest); found; found = ID.exec(rest)) {
      if (found.index > 0) nodes.push(rest.slice(0, found.index));
      nodes.push(known.has(found[1]) ? toPage(found[1], found[1]) : found[1]);
      rest = rest.slice(found.index + found[1].length);
    }
    if (rest !== '') nodes.push(rest);
    return nodes;
  });
}

/**
 * The nodes for `blocks`. `make` builds an element as dom.js's `h` does;
 * `known` is the set of error IDs that have a page, for the links.
 */
export function render(blocks, make, known = new Set()) {
  return blocks.map((block) => {
    if ('heading' in block) return make(`h${block.level}`, {}, ...inlineNodes(block.heading, make, known));
    if ('paragraph' in block) return make('p', {}, ...inlineNodes(block.paragraph, make, known));
    if ('code' in block) return make('pre', { class: 'log', tabindex: '0' }, make('code', {}, block.code));
    return make(
      block.ordered ? 'ol' : 'ul',
      {},
      ...block.items.map((item) => make('li', {}, ...inlineNodes(item, make, known))),
    );
  });
}
