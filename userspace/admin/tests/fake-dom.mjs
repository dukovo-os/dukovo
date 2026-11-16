// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// As much of a page as the console's views need to be built and pressed in a
// test: elements with attributes, children, listeners, a class list, focus,
// a value, dialogs that open and close, and the few selectors the views use
// (a tag, `.class`, `#id`, `[name]`, `[name="value"]`, joined, and a space
// for a descendant). Not a test itself, and never installed. What a page
// looks like is the lab's web-console tests.

/** A piece of text. */
class Text {
  constructor(text) {
    this.text = text;
    this.parent = null;
  }

  get textContent() {
    return this.text;
  }
}

/** The parts of a simple selector: `tag.class#id[name="value"]`. */
function partsOf(selector) {
  return [...selector.matchAll(/([.#]?)([\w-]+)|\[([\w-]+)(?:="([^"]*)")?\]/g)].map((found) => (found[3]
    ? { attribute: found[3], value: found[4] }
    : { kind: found[1], name: found[2] }));
}

export class Element {
  constructor(tag, document) {
    this.tag = tag;
    this.document = document;
    this.attributes = new Map();
    this.children = [];
    this.parent = null;
    this.listeners = new Map();
    this.value = '';
    this.open = false;
    this.returnValue = '';
    this.classes = new Set();
    this.classList = {
      add: (...names) => names.forEach((name) => this.classes.add(name)),
      remove: (...names) => names.forEach((name) => this.classes.delete(name)),
      contains: (name) => this.classes.has(name),
      toggle: (name, force = !this.classes.has(name)) => {
        if (force) this.classes.add(name);
        else this.classes.delete(name);
        return force;
      },
    };
  }

  setAttribute(name, value) {
    this.attributes.set(name, String(value));
    if (name === 'class') this.classes = new Set(String(value).split(/\s+/).filter(Boolean));
  }

  getAttribute(name) {
    return this.attributes.has(name) ? this.attributes.get(name) : null;
  }

  removeAttribute(name) {
    this.attributes.delete(name);
  }

  hasAttribute(name) {
    return this.attributes.has(name);
  }

  get id() {
    return this.getAttribute('id') ?? '';
  }

  get hidden() {
    return this.hasAttribute('hidden');
  }

  set hidden(on) {
    if (on) this.setAttribute('hidden', '');
    else this.removeAttribute('hidden');
  }

  /** Whether `node` is this element or lies under it. */
  contains(node) {
    return node === this || this.descendants.includes(node);
  }

  get isConnected() {
    let node = this;
    while (node.parent) node = node.parent;
    return node === this.document.body;
  }

  get textContent() {
    return this.children.map((child) => child.textContent).join('');
  }

  set textContent(text) {
    this.replaceChildren(text);
  }

  append(...nodes) {
    for (const node of nodes) {
      const child = typeof node === 'string' ? new Text(node) : node;
      child.parent?.children.splice(child.parent.children.indexOf(child), 1);
      child.parent = this;
      this.children.push(child);
    }
  }

  replaceChildren(...nodes) {
    for (const child of this.children) child.parent = null;
    this.children = [];
    this.append(...nodes);
  }

  remove() {
    this.parent?.children.splice(this.parent.children.indexOf(this), 1);
    this.parent = null;
  }

  addEventListener(type, listener) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }

  /** Calls the listeners of `type` with an event that says whether it was prevented, and gives the event. */
  fire(type, more = {}) {
    const event = {
      type, target: this, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; }, ...more,
    };
    for (const listener of this.listeners.get(type) ?? []) listener(event);
    return event;
  }

  matches(selector) {
    const parts = partsOf(selector);
    return parts.every((part) => {
      if (part.attribute) return this.hasAttribute(part.attribute) && (part.value === undefined || this.getAttribute(part.attribute) === part.value);
      if (part.kind === '.') return this.classes.has(part.name);
      if (part.kind === '#') return this.id === part.name;
      return this.tag === part.name;
    });
  }

  closest(selector) {
    for (let node = this; node instanceof Element; node = node.parent) if (node.matches(selector)) return node;
    return null;
  }

  /** Every element under this one, in document order. */
  get descendants() {
    return this.children.flatMap((child) => (child instanceof Element ? [child, ...child.descendants] : []));
  }

  querySelectorAll(selector) {
    const chain = selector.trim().split(/\s+/);
    return this.descendants.filter((node) => {
      if (!node.matches(chain.at(-1))) return false;
      let from = node.parent;
      for (const ancestor of chain.slice(0, -1).reverse()) {
        while (from instanceof Element && !from.matches(ancestor)) from = from.parent;
        if (!(from instanceof Element)) return false;
        from = from.parent;
      }
      return true;
    });
  }

  querySelector(selector) {
    return this.querySelectorAll(selector)[0] ?? null;
  }

  focus() {
    this.document.activeElement = this;
  }

  /** A press: a submit button of a form that closes its dialog does so with its value; any other button calls its listeners. */
  click() {
    if (this.hasAttribute('disabled')) return;
    const event = this.fire('click');
    const form = this.closest('form[method="dialog"]');
    if (!event.defaultPrevented && this.tag === 'button' && this.getAttribute('type') === 'submit' && form) {
      form.closest('dialog').close(this.getAttribute('value') ?? '');
    }
  }

  /** Opens the dialog, and gives focus to the element that asks for it with `autofocus`, as a browser does. */
  showModal() {
    this.open = true;
    this.querySelector('[autofocus]')?.focus();
  }

  close(value = this.returnValue) {
    if (!this.open) return;
    this.open = false;
    this.returnValue = value;
    this.fire('close');
  }
}

/**
 * A page with a head and a body, and the calls the views make on `document`.
 * A style sheet linked into the head loads a moment later, as one that comes
 * from the cache does, unless `loadsSheets` is false and the test fires the
 * link's `load` or `error` itself.
 */
export function makeDocument({ loadsSheets = true } = {}) {
  const document = {
    activeElement: null,
    createElement: (tag) => new Element(tag, document),
    createElementNS: (_, tag) => new Element(tag, document),
    getElementById: (id) => document.body.descendants.find((node) => node.id === id) ?? null,
    querySelector: (selector) => document.body.querySelector(selector),
  };
  document.head = new Element('head', document);
  if (loadsSheets) {
    const append = document.head.append.bind(document.head);
    document.head.append = (...nodes) => {
      append(...nodes);
      for (const node of nodes) if (node.tag === 'link') queueMicrotask(() => node.fire('load'));
    };
  }
  document.body = new Element('body', document);
  document.activeElement = document.body;
  return document;
}

/**
 * Makes a page the global one for the test `context`, with a clipboard that
 * holds what was copied (or refuses when `refuses`) and a selection that
 * remembers the node it was given, and gives `{document, copied, selected}`.
 * `loadsSheets` is makeDocument's. All of it is taken away when the test
 * ends.
 */
export function usePage(context, { refuses = false, loadsSheets = true } = {}) {
  const document = makeDocument({ loadsSheets });
  const copied = [];
  let selected = null;
  document.createRange = () => ({ selectNodeContents: (node) => { selected = node; } });
  const navigator = { clipboard: { writeText: async (text) => { if (refuses) throw new Error('not allowed'); copied.push(text); } } };
  const was = { document: globalThis.document, navigator: Object.getOwnPropertyDescriptor(globalThis, 'navigator'), getSelection: globalThis.getSelection };
  globalThis.document = document;
  globalThis.getSelection = () => ({ removeAllRanges() {}, addRange() {} });
  Object.defineProperty(globalThis, 'navigator', { value: navigator, configurable: true });
  context.after(() => {
    if (was.document === undefined) delete globalThis.document;
    else globalThis.document = was.document;
    if (was.navigator) Object.defineProperty(globalThis, 'navigator', was.navigator);
    else delete globalThis.navigator;
    if (was.getSelection === undefined) delete globalThis.getSelection;
    else globalThis.getSelection = was.getSelection;
  });
  return { document, copied, selected: () => selected };
}
