// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Forms from an action's input schema (design 11.5), as plain values. The
// schema is the part of JSON Schema 2020-12 that admind writes for its
// actions (admind/actions/mod.rs): an object with `properties`, `required`
// and `additionalProperties: false`, whose properties are strings (with
// `minLength`, `maxLength`, `pattern` or `enum`), integers (`minimum`,
// `maximum`) or booleans, each with a `description`. A schema with any
// other keyword has no form: `formFor` says why, and the view gives one
// text area for JSON instead.
//
// `checkArgs` holds arguments to the schema as admind's own check does
// (admind/actions/tests/validate.rs): a length is counted in characters,
// that is code points, a pattern matches anywhere in the text unless it
// says `^` and `$`, an enum names the words that go, and a number is a
// whole number. admind still decides; this is so that most mistakes show
// before a call, under the field they are about.

// What a schema may say of each kind of value.
const KEYWORDS = {
  object: ['type', 'properties', 'required', 'additionalProperties', 'description'],
  string: ['type', 'description', 'minLength', 'maxLength', 'pattern', 'enum'],
  integer: ['type', 'description', 'minimum', 'maximum'],
  boolean: ['type', 'description'],
};

/** An enum of this many words or fewer is a set of radio buttons; one of more is a menu. */
export const RADIO_WORDS = 4;

/** What the form says when it has no controls for a schema. */
export const JSON_NOTE = 'This action\'s arguments are not described in a form this console knows yet. Give them as JSON, as the API takes them.';

const isObject = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
const isCount = (value) => Number.isInteger(value) && value >= 0;

const patterns = new Map();

/** The regular expression for a schema's `pattern`, as a person leaving the field is checked with: Unicode mode; null when it does not read. */
export function patternOf(source) {
  if (!patterns.has(source)) {
    try {
      patterns.set(source, new RegExp(source, 'u'));
    } catch {
      patterns.set(source, null);
    }
  }
  return patterns.get(source);
}

/** The name of an argument as a label reads: "copy-on-select" is "Copy on select". */
export function labelOf(name) {
  const words = name.replace(/[-_]+/g, ' ').trim();
  return words ? words[0].toUpperCase() + words.slice(1) : name;
}

/** The label with the words that say the field is needed, as the form shows it. */
export const labelWords = (field) => (field.required ? `${field.label} (needed)` : field.label);

/** The problem of a property's schema that keeps it out of the subset, or null. */
function propertyProblem(name, schema) {
  if (!isObject(schema)) return `${name} is not a schema`;
  const allowed = Object.hasOwn(KEYWORDS, schema.type) ? KEYWORDS[schema.type] : null;
  if (!allowed || schema.type === 'object') return `${name} has the type ${JSON.stringify(schema.type)}`;
  const unknown = Object.keys(schema).find((key) => !allowed.includes(key));
  if (unknown) return `${name} has the keyword ${unknown}`;
  if ('description' in schema && typeof schema.description !== 'string') return `${name}'s description is not text`;
  for (const key of ['minLength', 'maxLength']) {
    if (key in schema && !isCount(schema[key])) return `${name}'s ${key} is not a count`;
  }
  for (const key of ['minimum', 'maximum']) {
    if (key in schema && !Number.isInteger(schema[key])) return `${name}'s ${key} is not a whole number`;
  }
  if ('pattern' in schema && (typeof schema.pattern !== 'string' || patternOf(schema.pattern) === null)) {
    return `${name}'s pattern does not read`;
  }
  if ('enum' in schema && (!Array.isArray(schema.enum) || schema.enum.length === 0 || !schema.enum.every((word) => typeof word === 'string'))) {
    return `${name}'s enum is not a list of words`;
  }
  return null;
}

/** The control a field is drawn as. */
function controlOf(schema) {
  if (schema.type === 'boolean') return 'checkbox';
  if (schema.type === 'integer') return 'number';
  if (schema.enum) return schema.enum.length <= RADIO_WORDS ? 'radio' : 'select';
  return 'input';
}

/** One field of the form, from the property `name` of `schema`, whose schema is `property`. */
function fieldOf(name, property, required) {
  return {
    name,
    label: labelOf(name),
    type: property.type,
    control: controlOf(property),
    required,
    description: property.description ?? '',
    minLength: property.minLength ?? null,
    maxLength: property.maxLength ?? null,
    pattern: property.pattern ?? null,
    words: property.enum ?? null,
    minimum: property.minimum ?? null,
    maximum: property.maximum ?? null,
  };
}

/**
 * The form for `schema`: `{ok: true, fields, description}`, a field for
 * each property in the order the schema gives them, or `{ok: false, why}`
 * when the schema has something this subset does not cover. A field is
 * `{name, label, type, control, required, description, minLength,
 * maxLength, pattern, words, minimum, maximum}`; `control` is `input`,
 * `radio`, `select`, `number` or `checkbox`.
 */
export function formFor(schema) {
  const no = (why) => ({ ok: false, why });
  if (!isObject(schema)) return no('the schema is not an object');
  if (schema.type !== 'object') return no(`the schema has the type ${JSON.stringify(schema.type)}, not object`);
  const unknown = Object.keys(schema).find((key) => !KEYWORDS.object.includes(key));
  if (unknown) return no(`the schema has the keyword ${unknown}`);
  if ('additionalProperties' in schema && schema.additionalProperties !== false) return no('the schema allows other arguments');
  const properties = schema.properties ?? {};
  if (!isObject(properties)) return no('the schema\'s properties are not an object');
  const required = schema.required ?? [];
  if (!Array.isArray(required) || !required.every((name) => typeof name === 'string' && Object.hasOwn(properties, name))) {
    return no('the schema\'s required list does not name properties');
  }
  const fields = [];
  for (const [name, property] of Object.entries(properties)) {
    const problem = propertyProblem(name, property);
    if (problem) return no(problem);
    fields.push(fieldOf(name, property, required.includes(name)));
  }
  return { ok: true, fields, description: typeof schema.description === 'string' ? schema.description : '' };
}

const characters = (text) => [...text].length;
const plural = (count, one, other) => `${count} ${count === 1 ? one : other}`;

/** The words that say what is wrong with a field, for each rule `checkArgs` names. */
export function wordsFor(field, rule) {
  const label = field.label;
  switch (rule) {
    case 'required': return `${label} is needed.`;
    case 'not-text': return `${label} is text.`;
    case 'too-short': return `${label} needs at least ${plural(field.minLength, 'character', 'characters')}.`;
    case 'too-long': return `${label} has at most ${plural(field.maxLength, 'character', 'characters')}.`;
    case 'pattern': return `${label} is not in a form this action takes. The hint under it says what it is.`;
    case 'not-a-word': return `${label} is one of: ${field.words.join(', ')}.`;
    case 'not-whole': return `${label} is a whole number.`;
    case 'below': return `${label} is at least ${field.minimum}.`;
    case 'above': return `${label} is at most ${field.maximum}.`;
    case 'not-flag': return `${label} is true or false.`;
    default: return `${label} cannot be used.`;
  }
}

/** The first rule that `value` breaks for `field`, as admind checks it, or null. */
export function ruleBroken(field, value) {
  switch (field.type) {
    case 'string': {
      if (typeof value !== 'string') return 'not-text';
      const length = characters(value);
      if (field.minLength !== null && length < field.minLength) return 'too-short';
      if (field.maxLength !== null && length > field.maxLength) return 'too-long';
      if (field.pattern !== null && !patternOf(field.pattern).test(value)) return 'pattern';
      if (field.words !== null && !field.words.includes(value)) return 'not-a-word';
      return null;
    }
    case 'integer':
      if (!Number.isInteger(value)) return 'not-whole';
      if (field.minimum !== null && value < field.minimum) return 'below';
      if (field.maximum !== null && value > field.maximum) return 'above';
      return null;
    default:
      return typeof value === 'boolean' ? null : 'not-flag';
  }
}

/**
 * Every problem that `args`, a plain object, has against `form`, as
 * `{field, rule, words}`, in the order of the fields: an argument that is
 * needed and not given, one that breaks a rule, and, last, a name that is
 * none of the arguments.
 */
export function checkArgs(form, args) {
  const problems = [];
  for (const field of form.fields) {
    const given = args[field.name];
    if (given === undefined || given === null) {
      if (field.required) problems.push({ field: field.name, rule: 'required', words: wordsFor(field, 'required') });
      continue;
    }
    const rule = ruleBroken(field, given);
    if (rule) problems.push({ field: field.name, rule, words: wordsFor(field, rule) });
  }
  const known = new Set(form.fields.map((field) => field.name));
  for (const name of Object.keys(args)) {
    if (!known.has(name)) problems.push({ field: name, rule: 'unknown', words: `${name} is not one of the arguments of this action.` });
  }
  return problems;
}

/** Whether `args` are all that the action needs and fit its schema. */
export const isReady = (form, args) => checkArgs(form, args).length === 0;

/**
 * What the controls hold for `args`, as the form starts: each field's text
 * (a checkbox's state as a boolean), for the controls to show.
 */
export function startingValues(form, args = {}) {
  return Object.fromEntries(form.fields.map((field) => {
    const given = args[field.name];
    if (field.type === 'boolean') return [field.name, given === true];
    return [field.name, given === undefined || given === null ? '' : String(given)];
  }));
}

/**
 * The arguments that the controls' `values` (as `startingValues` gives
 * them) stand for. A field left empty is left out, unless it is a text that
 * needs no characters; a checkbox always gives true or false; a number's
 * text becomes a number when it is one, and stays text when it is not, so
 * that `checkArgs` says it is no whole number.
 */
export function argsFrom(form, values) {
  const args = {};
  for (const field of form.fields) {
    const held = values[field.name];
    if (field.type === 'boolean') {
      args[field.name] = held === true;
    } else if (held === '' || held === undefined) {
      if (field.type === 'string' && field.required && field.minLength === 0 && field.words === null) args[field.name] = '';
    } else if (field.type === 'integer') {
      const number = Number(held);
      args[field.name] = String(held).trim() !== '' && Number.isFinite(number) ? number : held;
    } else {
      args[field.name] = held;
    }
  }
  return args;
}

/**
 * The arguments that a text of JSON stands for, for a schema with no form:
 * `{args}`, or `{problem}` in words when the text is not a JSON object. An
 * empty text is no arguments.
 */
export function parseJsonArgs(text) {
  if (text.trim() === '') return { args: {} };
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch {
    return { problem: 'This is not JSON. Write the arguments as a JSON object, such as {"name": "nginx"}.' };
  }
  if (!isObject(parsed)) return { problem: 'The arguments are a JSON object, such as {"name": "nginx"}.' };
  return { args: parsed };
}
