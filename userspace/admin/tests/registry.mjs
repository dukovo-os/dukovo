// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The input schema of each action of admind's registry, read from the Rust
// source of admind/actions/ as the build reads it, so that a test of the
// console's forms never holds a copy of an action's arguments. Not a test
// itself, and never installed.
//
// The reader knows the few shapes the registry is written in: the action
// types listed by `registry!`, each with `type Args`, either in its own
// `impl Action` or as the arguments in a `change!` macro call; the
// arguments' `FIELDS`, built with `Field::text`, `one_of`, `number`, `flag`
// and `secret` (and `.optional()`), possibly through a `const fn` of the file;
// sums and products of whole numbers; and constants of the same file, or of
// the other crates it takes them from, in the places `CRATES` names.
// Anything else stops the reader with a message that says where, so that a
// new shape is taught here rather than skipped.

import { readFileSync } from 'node:fs';

const REPO = new URL('../../../', import.meta.url);
const ACTIONS = 'userspace/programs/systools/src/admind/actions/';

// Where the crates that the actions take constants from keep them.
const CRATES = {
  'radix_abi::resource_domain': 'abi/resource_domain.ridl',
  'manifest::service': 'userspace/programs/manifest/src/service.rs',
  // The accounts library's limits: `accounts::NAME_MAX`.
  accounts: 'userspace/programs/accounts/src/lib.rs',
  // systools' own log(1), as `use crate::log` names it.
  log: 'userspace/programs/systools/src/log.rs',
  // update(8)'s checks' limits, which the updates' actions hold a check to: `checks::NAME_MAX`.
  checks: 'userspace/programs/systools/src/update/checks.rs',
  // wireguard(8)'s readers, which the private access actions check with.
  'crate::private_access': 'userspace/programs/systools/src/private_access.rs',
  // What an action takes from another action file: `use super::metrics::WORDS`.
  'super::accounts': `${ACTIONS}accounts.rs`,
  'super::apps': `${ACTIONS}apps.rs`,
  'super::backups': `${ACTIONS}backups.rs`,
  'super::metrics': `${ACTIONS}metrics.rs`,
  'super::snapshots': `${ACTIONS}snapshots.rs`,
  'super::trial': `${ACTIONS}trial.rs`,
};

const read = (path) => readFileSync(new URL(path, REPO), 'utf8');

/** The tokens of Rust source, without comments: `{kind, value, at}` with kind `str`, `num`, `id` or `p`. */
function tokenize(text) {
  const tokens = [];
  let at = 0;
  const escapes = { n: '\n', t: '\t', r: '\r', 0: '\0', '\\': '\\', '"': '"', "'": "'" };
  while (at < text.length) {
    const here = text[at];
    if (/\s/.test(here)) {
      at += 1;
    } else if (text.startsWith('//', at)) {
      const end = text.indexOf('\n', at);
      at = end === -1 ? text.length : end;
    } else if (text.startsWith('/*', at)) {
      const end = text.indexOf('*/', at + 2);
      at = end === -1 ? text.length : end + 2;
    } else if (/^r#*"/.test(text.slice(at, at + 8))) {
      // A raw string, r"..." or r#"..."#, holds its text as it is.
      const hashes = /^r(#*)"/.exec(text.slice(at, at + 8))[1];
      const close = `"${hashes}`;
      const begin = at + 2 + hashes.length;
      const end = text.indexOf(close, begin);
      const stop = end === -1 ? text.length : end;
      tokens.push({ kind: 'str', value: text.slice(begin, stop), at });
      at = stop + close.length;
    } else if (here === '"') {
      let value = '';
      const start = at;
      at += 1;
      while (text[at] !== '"') {
        if (text[at] === '\\') {
          const next = text[at + 1];
          if (next === '\n') {
            at += 2;
            while (/\s/.test(text[at])) at += 1;
          } else if (next === 'u') {
            const end = text.indexOf('}', at);
            value += String.fromCodePoint(Number.parseInt(text.slice(at + 3, end), 16));
            at = end + 1;
          } else if (next === 'x') {
            value += String.fromCharCode(Number.parseInt(text.slice(at + 2, at + 4), 16));
            at += 4;
          } else if (next in escapes) {
            value += escapes[next];
            at += 2;
          } else {
            throw new Error(`the registry reader does not know the escape \\${next} in ${text.slice(start, start + 40)}`);
          }
        } else {
          value += text[at];
          at += 1;
        }
      }
      tokens.push({ kind: 'str', value, at: start });
      at += 1;
    } else if (/\d/.test(here)) {
      const found = /^\d[\d_]*/.exec(text.slice(at))[0];
      tokens.push({ kind: 'num', value: Number(found.replaceAll('_', '')), at });
      at += found.length;
    } else if (/[A-Za-z_$]/.test(here)) {
      const found = /^[A-Za-z_$][A-Za-z0-9_$]*/.exec(text.slice(at))[0];
      tokens.push({ kind: 'id', value: found, at });
      at += found.length;
    } else if (here === "'") {
      // A character such as '/' is skipped, and so is a lifetime such as 'static.
      const found = /^'(?:\\.|[^\\'])'|^'[A-Za-z_]+/.exec(text.slice(at));
      if (!found) throw new Error(`the registry reader does not know the quote at ${text.slice(at, at + 20)}`);
      at += found[0].length;
    } else if (text.startsWith('::', at)) {
      tokens.push({ kind: 'p', value: '::', at });
      at += 2;
    } else {
      tokens.push({ kind: 'p', value: here, at });
      at += 1;
    }
  }
  return tokens;
}

/** A source file: its text and tokens, and the names it imports with `use`. */
class Source {
  constructor(name, text) {
    this.name = name;
    // What the registry says comes before each file's tests, whose strings
    // the reader need not know; `mod tests;` in another file cuts nothing.
    this.text = text.split(/^#\[cfg\(test\)\](?!\s*mod \w+;)/m)[0];
    this.tokens = tokenize(this.text);
    this.imports = new Map();
    for (const [, path, names] of text.matchAll(/^use\s+([\w:]+)::\{([^}]*)\};/gm)) {
      for (const name of names.split(',').map((one) => one.trim()).filter(Boolean)) this.imports.set(name, path);
    }
    for (const [, path, name] of text.matchAll(/^use\s+([\w:]+)::(\w+);/gm)) this.imports.set(name, path);
  }

  /** The index of the first token that is `value` at or after `from`, or -1. */
  find(value, from = 0) {
    return this.tokens.findIndex((token, index) => index >= from && token.kind !== 'str' && token.value === value);
  }
}

const sources = new Map();
function source(path) {
  if (!sources.has(path)) sources.set(path, new Source(path, read(path)));
  return sources.get(path);
}

/**
 * Parses an expression of `file` from token `start`, and says where it ends:
 * `{value, end}`. `env` maps the parameters of a function being read to the
 * values it was called with.
 */
function parseAt(file, start, env = new Map()) {
  let at = start;
  const peek = () => file.tokens[at];
  const take = (value) => {
    const token = file.tokens[at];
    if (value !== undefined && token?.value !== value) {
      throw new Error(`${file.name}: expected ${value} at "${file.text.slice(token?.at ?? file.text.length, (token?.at ?? 0) + 40)}"`);
    }
    at += 1;
    return token;
  };

  function list(close) {
    const items = [];
    while (peek().value !== close) {
      items.push(expression());
      if (peek().value === ',') take(',');
    }
    take(close);
    return items;
  }

  function path() {
    const names = [take().value];
    while (peek()?.value === '::') {
      take('::');
      names.push(take().value);
    }
    return names;
  }

  function primary() {
    const token = peek();
    if (token.kind === 'str' || token.kind === 'num') return take().value;
    if (token.value === '&') {
      take('&');
      return primary();
    }
    if (token.value === '[') {
      take('[');
      return list(']');
    }
    if (token.value === '(') {
      take('(');
      // `(PATH_MAX + 1)` is a number in brackets; a tuple has a comma.
      const items = list(')');
      return items.length === 1 ? items[0] : items;
    }
    if (token.kind !== 'id') throw new Error(`${file.name}: no expression at "${file.text.slice(token.at, token.at + 40)}"`);
    const names = path();
    if (peek()?.value === '(') {
      take('(');
      return call(names, list(')'), file);
    }
    if (names.length === 1 && env.has(names[0])) return env.get(names[0]);
    return constant(names, file);
  }

  /** A product, `PATHS_MOST * (PATH_MAX + 1)`, of whole numbers. */
  function product() {
    let value = postfix();
    while (peek()?.value === '*') {
      take('*');
      value *= postfix();
    }
    return value;
  }

  /** A sum or difference of products, as a limit of the registry is written. */
  function expression() {
    let value = product();
    while (peek()?.value === '+' || peek()?.value === '-') {
      const sign = take().value === '+' ? 1 : -1;
      value += sign * product();
    }
    return value;
  }

  function postfix() {
    let value = primary();
    for (;;) {
      const token = peek();
      if (token?.value === '[') {
        take('[');
        value = value[expression()];
        take(']');
      } else if (token?.value === '.') {
        take('.');
        const next = take();
        if (next.kind === 'num') {
          value = value[next.value];
        } else if (next.value === 'optional') {
          take('(');
          take(')');
          value = { ...value, required: false };
        } else {
          throw new Error(`${file.name}: the registry reader does not know the method ${next.value}`);
        }
      } else if (token?.kind === 'id' && token.value === 'as') {
        take();
        take();
      } else {
        return value;
      }
    }
  }

  const value = expression();
  return { value, end: at };
}

/** The value of the constant `names` (a path) named in `file`. */
function constant(names, file) {
  const name = names.at(-1);
  if (names.length === 1 && name === 'None') return null;
  let home = file;
  let path = names.slice(0, -1).join('::');
  if (path === '' && file.imports.has(name)) path = file.imports.get(name);
  if (path !== '') {
    if (!(path in CRATES)) throw new Error(`${file.name}: the registry reader does not know where ${path}::${name} is kept`);
    home = source(CRATES[path]);
  }
  for (let at = home.find('const'); at !== -1; at = home.find('const', at + 1)) {
    if (home.tokens[at + 1].value !== name || home.tokens[at + 2].value !== ':') continue;
    let depth = 0;
    let equals = at + 3;
    for (; ; equals += 1) {
      const token = home.tokens[equals];
      if ('[('.includes(token.value) && token.kind === 'p') depth += 1;
      if ('])'.includes(token.value) && token.kind === 'p') depth -= 1;
      if (token.value === '=' && depth === 0) break;
    }
    return parseAt(home, equals + 1).value;
  }
  throw new Error(`${file.name}: no constant ${name}`);
}

/** The value of a call: a `Field` constructor, `Some`, or a `const fn` of `file`. */
function call(names, args, file) {
  const kind = names.join('::');
  switch (kind) {
    case 'Some': return args[0];
    case 'Field::text': {
      const [name, about, min, max, pattern] = args;
      return { name, about, required: true, type: 'string', minLength: min, maxLength: max, pattern };
    }
    case 'Field::one_of': return { name: args[0], about: args[1], required: true, type: 'string', words: args[2] };
    case 'Field::number': return { name: args[0], about: args[1], required: true, type: 'integer', minimum: args[2], maximum: args[3] };
    case 'Field::flag': return { name: args[0], about: args[1], required: true, type: 'boolean' };
    // One line of text, sent and never given back (ADR 0561), as mod.rs's schema says.
    case 'Field::secret': return { name: args[0], about: args[1], required: true, type: 'string', minLength: 1, maxLength: args[2], pattern: '^[^\n\r\0]*$', writeOnly: true };
    default: break;
  }
  if (names.length === 1) return callFn(names[0], args, file);
  throw new Error(`${file.name}: the registry reader does not know the call ${kind}`);
}

/** A `const fn NAME(PARAMS) -> Field { ... }` of `file`, called with `args`. */
function callFn(name, args, file) {
  for (let at = file.find('fn'); at !== -1; at = file.find('fn', at + 1)) {
    if (file.tokens[at + 1].value !== name) continue;
    const params = [];
    let close = at + 3;
    for (; file.tokens[close].value !== ')'; close += 1) {
      if (file.tokens[close + 1].value === ':') params.push(file.tokens[close].value);
    }
    const body = file.find('{', close) + 1;
    return parseAt(file, body, new Map(params.map((param, index) => [param, args[index]]))).value;
  }
  throw new Error(`${file.name}: no function ${name}`);
}

/** The id list of `registry!`: `[module, Type]` for each action type, in order. */
function registered(mod) {
  const start = mod.find('registry', 0);
  const found = [];
  for (let at = mod.find('[', mod.find('!', start)) + 1; mod.tokens[at].value !== ']'; at += 1) {
    if (mod.tokens[at].kind === 'id' && mod.tokens[at + 1].value === '::') {
      found.push([mod.tokens[at].value, mod.tokens[at + 2].value]);
      at += 2;
    }
  }
  return found;
}

/** The type that is the arguments of the action `type` of `file`, and the id: from its `impl Action`, or from a `change!` call. */
function actionOf(file, type) {
  for (let at = file.find('impl'); at !== -1; at = file.find('impl', at + 1)) {
    const t = file.tokens;
    if (t[at + 1].value !== 'Action' || t[at + 2].value !== 'for' || t[at + 3].value !== type) continue;
    const id = idIn(file, at);
    const args = file.tokens[file.find('Args', at) + 2];
    return { id, args: args.value };
  }
  for (let at = file.find('change'); at !== -1; at = file.find('change', at + 1)) {
    const t = file.tokens;
    if (t[at + 1].value !== '!' || t[at + 3].value !== type) continue;
    // change! { Type, "verb", "Title", Args, ... }
    return { id: `service.${t[at + 5].value}`, args: t[at + 9].value };
  }
  throw new Error(`${file.name}: no action ${type}`);
}

/** The text of the `const ID` after token `at`. */
function idIn(file, at) {
  const from = file.find('ID', at);
  const equals = file.find('=', from);
  const token = file.tokens[equals + 1];
  if (token.kind !== 'str') throw new Error(`${file.name}: the registry reader reads ID only as a string`);
  return token.value;
}

/** The fields of the arguments type `type` in `file`: `[]` for `Nothing`. */
function fieldsOf(file, type) {
  if (type === 'Nothing') return [];
  for (let at = file.find('impl'); at !== -1; at = file.find('impl', at + 1)) {
    const t = file.tokens;
    if (t[at + 1].value !== 'Args' || t[at + 2].value !== 'for' || t[at + 3].value !== type) continue;
    const from = file.find('FIELDS', at);
    return parseAt(file, file.find('=', from) + 1).value;
  }
  throw new Error(`${file.name}: no impl Args for ${type}`);
}

/** The JSON Schema that `Field`s make, as `schema()` in mod.rs writes it. */
function schemaOf(fields) {
  const properties = {};
  for (const field of fields) {
    const property = {};
    if (field.type === 'string' && field.words) {
      property.enum = field.words;
    } else if (field.type === 'string') {
      property.minLength = field.minLength;
      property.maxLength = field.maxLength;
      if (field.pattern !== null) property.pattern = field.pattern;
    } else if (field.type === 'integer') {
      property.minimum = field.minimum;
      property.maximum = field.maximum;
    }
    property.type = field.type;
    property.description = field.about;
    properties[field.name] = property;
  }
  return {
    type: 'object',
    properties,
    required: fields.filter((field) => field.required).map((field) => field.name),
    additionalProperties: false,
  };
}

/** The ids of the actions that `lab/actions.toml` lists, in order. */
export function listedIds() {
  return [...read('lab/actions.toml').matchAll(/^id = "([^"]+)"/gm)].map((found) => found[1]);
}

/**
 * The list `GET /api/1/actions` gives, as far as the pages read it: each
 * action `lab/actions.toml` lists, with its title, area, method, address and
 * who runs it, and its input schema from the registry.
 */
export function listedActions() {
  const schemas = inputSchemas();
  return read('lab/actions.toml').split('[[action]]').slice(1).map((block) => {
    const value = (name) => new RegExp(`^${name} = "([^"]*)"`, 'm').exec(block)?.[1];
    const id = value('id');
    return {
      id,
      title: value('title'),
      area: value('area'),
      changes: /^changes = true/m.test(block),
      method: value('method'),
      path: value('path'),
      runs: value('runs'),
      long: /^long = true/m.test(block),
      input: schemas.get(id),
    };
  });
}

/**
 * Every action the registry has, and the lab's two, as a Map of id to the
 * schema of its input. The registry's order is the one `lab/actions.toml`
 * lists them in.
 */
export function inputSchemas() {
  const mod = source(`${ACTIONS}mod.rs`);
  const found = new Map();
  const types = registered(mod);
  types.push(...[...mod.text.matchAll(/Entry<lab::(\w+)>/g)].map((one) => ['lab', one[1]]));
  for (const [name, type] of types) {
    const file = source(`${ACTIONS}${name}.rs`);
    const { id, args } = actionOf(file, type);
    // `Nothing` is in mod.rs; every other arguments type is in the action's file.
    found.set(id, schemaOf(fieldsOf(file, args)));
  }
  return found;
}
