// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// lib/schema-model.js (design 11.5): each keyword of the schema subset, the
// fallback for a schema outside it, and the check of arguments the way
// admind's own validator makes it (admind/actions/tests/validate.rs). The
// last tests take the schema of every action from admind's registry, read
// from the Rust source by tests/registry.mjs, and check that each makes a
// form and that the form holds the arguments the actions hold.

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  argsFrom, checkArgs, formFor, isReady, JSON_NOTE, labelOf, labelWords, parseJsonArgs, patternOf, RADIO_WORDS,
  ruleBroken, startingValues, wordsFor,
} from '../lib/schema-model.js';
import { inputSchemas, listedIds } from './registry.mjs';

const object = (properties, required = [], more = {}) => ({
  type: 'object', properties, required, additionalProperties: false, ...more,
});
const only = (property, required = true) => formFor(object({ x: property }, required ? ['x'] : [])).fields[0];
const rules = (form, args) => checkArgs(form, args).map((problem) => `${problem.field}:${problem.rule}`);

test('a schema that takes nothing is a form with no fields', () => {
  const form = formFor(object({}));
  assert.deepEqual(form, { ok: true, fields: [], description: '' });
  assert.deepEqual(checkArgs(form, {}), []);
});

test('a string has a text control that holds its length, pattern and description', () => {
  const field = only({
    type: 'string', description: 'A name.', minLength: 1, maxLength: 23, pattern: '^[a-z]+$',
  });
  assert.deepEqual(field, {
    name: 'x', label: 'X', type: 'string', control: 'input', required: true, description: 'A name.',
    minLength: 1, maxLength: 23, pattern: '^[a-z]+$', words: null, minimum: null, maximum: null,
  });
  assert.equal(labelWords(field), 'X (needed)');
  assert.equal(labelWords(only({ type: 'string' }, false)), 'X');
});

test('an enum of four words or fewer is radio buttons and one of more is a menu', () => {
  assert.equal(RADIO_WORDS, 4);
  assert.equal(only({ type: 'string', enum: ['a'] }).control, 'radio');
  assert.equal(only({ type: 'string', enum: ['a', 'b', 'c', 'd'] }).control, 'radio');
  assert.equal(only({ type: 'string', enum: ['a', 'b', 'c', 'd', 'e'] }).control, 'select');
  assert.deepEqual(only({ type: 'string', enum: ['private', 'any'] }).words, ['private', 'any']);
});

test('an integer is a number control with its limits, and a boolean a checkbox', () => {
  const number = only({ type: 'integer', minimum: 1, maximum: 120 }, false);
  assert.equal(number.control, 'number');
  assert.equal(number.minimum, 1);
  assert.equal(number.maximum, 120);
  assert.equal(number.required, false);
  assert.equal(only({ type: 'boolean', description: 'Whether.' }).control, 'checkbox');
});

test('the fields come in the order of the properties, and a name is made into a label', () => {
  const form = formFor(object({ 'copy-on-select': { type: 'boolean' }, keep_days: { type: 'integer' }, a: { type: 'string' } }, ['a']));
  assert.deepEqual(form.fields.map((field) => [field.name, field.label, field.required]), [
    ['copy-on-select', 'Copy on select', false], ['keep_days', 'Keep days', false], ['a', 'A', true],
  ]);
  assert.equal(labelOf('range'), 'Range');
  assert.equal(labelOf('-'), '-');
});

test('a schema that the subset does not cover has no form, and says what stops it', () => {
  const why = (schema) => {
    const form = formFor(schema);
    assert.equal(form.ok, false, JSON.stringify(schema));
    return form.why;
  };
  assert.match(why(null), /not an object/);
  assert.match(why([]), /not an object/);
  assert.match(why({ type: 'array' }), /type "array"/);
  assert.match(why({ type: 'object', oneOf: [] }), /keyword oneOf/);
  assert.match(why(object({}, [], { $schema: 'https://json-schema.org/draft/2020-12/schema' })), /keyword \$schema/);
  assert.match(why(object({}, [], { additionalProperties: true })), /allows other arguments/);
  assert.match(why({ type: 'object', properties: [] }), /properties are not an object/);
  assert.match(why(object({ a: {} }, ['b'])), /required list/);
  assert.match(why(object({ a: { type: 'string' } }, 'a')), /required list/);
  assert.match(why(object({ a: 5 })), /a is not a schema/);
  assert.match(why(object({ a: { type: 'object' } })), /a has the type "object"/);
  assert.match(why(object({ a: { type: 'array' } })), /a has the type "array"/);
  assert.match(why(object({ a: { type: 'constructor' } })), /a has the type "constructor"/);
  assert.match(why(object({ a: { type: 'string', format: 'uri' } })), /a has the keyword format/);
  assert.match(why(object({ a: { type: 'string', default: 'x' } })), /a has the keyword default/);
  assert.match(why(object({ a: { type: 'string', minimum: 1 } })), /a has the keyword minimum/);
  assert.match(why(object({ a: { type: 'integer', pattern: 'x' } })), /a has the keyword pattern/);
  assert.match(why(object({ a: { type: 'boolean', enum: [true] } })), /a has the keyword enum/);
  assert.match(why(object({ a: { type: 'string', description: 5 } })), /description is not text/);
  assert.match(why(object({ a: { type: 'string', minLength: -1 } })), /minLength is not a count/);
  assert.match(why(object({ a: { type: 'string', maxLength: 1.5 } })), /maxLength is not a count/);
  assert.match(why(object({ a: { type: 'integer', minimum: 0.5 } })), /minimum is not a whole number/);
  assert.match(why(object({ a: { type: 'integer', maximum: '5' } })), /maximum is not a whole number/);
  assert.match(why(object({ a: { type: 'string', pattern: '(' } })), /pattern does not read/);
  assert.match(why(object({ a: { type: 'string', pattern: 5 } })), /pattern does not read/);
  assert.match(why(object({ a: { type: 'string', enum: [] } })), /enum is not a list of words/);
  assert.match(why(object({ a: { type: 'string', enum: [1, 2] } })), /enum is not a list of words/);
  assert.match(why(object({ a: { type: 'string', enum: 'ab' } })), /enum is not a list of words/);
  assert.match(JSON_NOTE, /JSON/);
});

test('an object and its fields may carry a description, and a schema that leaves out required or additionalProperties is read', () => {
  const form = formFor({ type: 'object', description: 'What it takes.', properties: { a: { type: 'string' } } });
  assert.equal(form.ok, true);
  assert.equal(form.description, 'What it takes.');
  assert.equal(form.fields[0].required, false);
  assert.equal(formFor({ type: 'object' }).ok, true);
});

test('a pattern is read in Unicode mode, and is null when it does not read', () => {
  assert.ok(patternOf('^a+$').test('aaa'));
  assert.equal(patternOf('(?!'), null);
  assert.equal(patternOf('\\-'), null);
  assert.equal(patternOf('^a+$'), patternOf('^a+$'));
});

// The cases of the Rust validator's own tests, in its words.

test('a string is held to its length in characters, its pattern and its words, as admind\'s validator holds it', () => {
  const text = formFor(object({ x: { type: 'string', minLength: 1, maxLength: 3, pattern: '^[a-z]*$' } }, ['x']));
  assert.deepEqual(rules(text, { x: 'abc' }), []);
  assert.deepEqual(rules(text, { x: '' }), ['x:too-short']);
  assert.deepEqual(rules(text, { x: 'abcd' }), ['x:too-long']);
  assert.deepEqual(rules(text, { x: 'AB' }), ['x:pattern']);
  assert.deepEqual(rules(text, { x: 7 }), ['x:not-text']);
  assert.deepEqual(rules(text, { x: 'ABCD' }), ['x:too-long'], 'the first rule that fails is the one named');
  const wide = formFor(object({ x: { type: 'string', maxLength: 3 } }, ['x']));
  assert.deepEqual(rules(wide, { x: 'ééé' }), []);
  assert.deepEqual(rules(wide, { x: '\u{1F642}\u{1F642}\u{1F642}' }), [], 'a code point outside the basic plane is one character');
  assert.deepEqual(rules(wide, { x: 'éééé' }), ['x:too-long']);
  const words = formFor(object({ x: { type: 'string', enum: ['a', 'b'] } }, ['x']));
  assert.deepEqual(rules(words, { x: 'b' }), []);
  assert.deepEqual(rules(words, { x: 'c' }), ['x:not-a-word']);
  assert.deepEqual(rules(words, { x: null }), ['x:required']);
  assert.deepEqual(rules(words, { x: ' b' }), ['x:not-a-word']);
});

test('a pattern matches anywhere in the text unless it says where, and a lookahead looks without taking', () => {
  const check = (pattern, value) => rules(formFor(object({ x: { type: 'string', pattern } }, ['x'])), { x: value }).length === 0;
  assert.equal(check('ab', 'xabx'), true);
  assert.equal(check('^ab', 'xab'), false);
  assert.equal(check('ab$', 'abx'), false);
  assert.equal(check('^ab$', 'ab'), true);
  assert.equal(check('^ab$', 'ab\n'), false);
  assert.equal(check('^(?!(check|new)$)[a-z]+$', 'checks'), true);
  assert.equal(check('^(?!(check|new)$)[a-z]+$', 'check'), false);
  assert.equal(check('^(?!.*\\.\\.)[a-z.]+$', 'a..b'), false);
});

test('a number is a whole number within its limits, and a flag is true or false', () => {
  const number = formFor(object({ x: { type: 'integer', minimum: 1, maximum: 5 } }, ['x']));
  for (const good of [1, 3, 5]) assert.deepEqual(rules(number, { x: good }), []);
  assert.deepEqual(rules(number, { x: 0 }), ['x:below']);
  assert.deepEqual(rules(number, { x: 6 }), ['x:above']);
  assert.deepEqual(rules(number, { x: 1.5 }), ['x:not-whole']);
  assert.deepEqual(rules(number, { x: '3' }), ['x:not-whole']);
  assert.deepEqual(rules(number, { x: true }), ['x:not-whole']);
  const flag = formFor(object({ x: { type: 'boolean' } }, ['x']));
  assert.deepEqual(rules(flag, { x: false }), []);
  assert.deepEqual(rules(flag, { x: 'yes' }), ['x:not-flag']);
});

test('what is needed and not given is a problem, an optional argument may be left out, and a name that is none of the arguments is refused', () => {
  const form = formFor(object({ a: { type: 'string' }, b: { type: 'integer' }, c: { type: 'boolean' } }, ['a', 'c']));
  assert.deepEqual(rules(form, {}), ['a:required', 'c:required']);
  assert.deepEqual(rules(form, { a: 'x', c: false }), []);
  assert.deepEqual(rules(form, { a: 'x', c: false, more: 1 }), ['more:unknown']);
  assert.deepEqual(rules(form, { a: null, b: null, c: true }), ['a:required'], 'null is read as left out');
  assert.equal(isReady(form, { a: 'x', c: true }), true);
  assert.equal(isReady(form, { a: 'x' }), false);
});

test('each problem is in words that name the field and what to do, in the console\'s plain voice', () => {
  const form = formFor(object({
    name: { type: 'string', minLength: 3, maxLength: 5, pattern: '^[a-z]+$' },
    reach: { type: 'string', enum: ['private', 'any'] },
    seconds: { type: 'integer', minimum: 1, maximum: 120 },
    force: { type: 'boolean' },
  }, ['name']));
  const [name, reach, seconds, force] = form.fields;
  assert.equal(wordsFor(name, 'required'), 'Name is needed.');
  assert.equal(wordsFor(name, 'not-text'), 'Name is text.');
  assert.equal(wordsFor(name, 'too-short'), 'Name needs at least 3 characters.');
  assert.equal(wordsFor({ ...name, minLength: 1 }, 'too-short'), 'Name needs at least 1 character.');
  assert.equal(wordsFor(name, 'too-long'), 'Name has at most 5 characters.');
  assert.equal(wordsFor({ ...name, maxLength: 1 }, 'too-long'), 'Name has at most 1 character.');
  assert.match(wordsFor(name, 'pattern'), /^Name is not in a form this action takes\./);
  assert.equal(wordsFor(reach, 'not-a-word'), 'Reach is one of: private, any.');
  assert.equal(wordsFor(seconds, 'not-whole'), 'Seconds is a whole number.');
  assert.equal(wordsFor(seconds, 'below'), 'Seconds is at least 1.');
  assert.equal(wordsFor(seconds, 'above'), 'Seconds is at most 120.');
  assert.equal(wordsFor(force, 'not-flag'), 'Force is true or false.');
  assert.equal(wordsFor(force, 'something-else'), 'Force cannot be used.');
  assert.deepEqual(checkArgs(form, { name: 'ab' }).map((problem) => problem.words), ['Name needs at least 3 characters.']);
  assert.equal(ruleBroken(name, 'abcd'), null);
});

test('the controls start from the arguments the thing gave, and give back the arguments they stand for', () => {
  const form = formFor(object({
    name: { type: 'string' }, note: { type: 'string' }, seconds: { type: 'integer' }, force: { type: 'boolean' },
  }, ['name']));
  assert.deepEqual(startingValues(form, { name: 'nginx', seconds: 20, force: true }), {
    name: 'nginx', note: '', seconds: '20', force: true,
  });
  assert.deepEqual(startingValues(form), { name: '', note: '', seconds: '', force: false });
  assert.deepEqual(argsFrom(form, { name: 'nginx', note: '', seconds: '', force: false }), { name: 'nginx', force: false });
  assert.deepEqual(argsFrom(form, { name: 'nginx', note: 'hi', seconds: '30', force: true }), {
    name: 'nginx', note: 'hi', seconds: 30, force: true,
  });
  // Text that is no number stays text, so that the check says it is no whole number.
  assert.deepEqual(argsFrom(form, { name: 'a', note: '', seconds: 'x', force: false }).seconds, 'x');
  assert.deepEqual(argsFrom(form, { name: 'a', note: '', seconds: ' ', force: false }).seconds, ' ');
  assert.deepEqual(rules(form, argsFrom(form, { name: 'a', note: '', seconds: '1.5', force: false })), ['seconds:not-whole']);
  // What the person typed is not trimmed: admind would refuse the space, so the form does too.
  assert.deepEqual(argsFrom(form, { name: ' a', note: '', seconds: '', force: false }).name, ' a');
});

test('a required text that may be empty is given empty, and a required choice that was not made is left out', () => {
  const form = formFor(object({ note: { type: 'string', minLength: 0 }, reach: { type: 'string', enum: ['a', 'b'] } }, ['note', 'reach']));
  const args = argsFrom(form, { note: '', reach: '' });
  assert.deepEqual(args, { note: '' });
  assert.deepEqual(rules(form, args), ['reach:required']);
});

test('arguments for a schema with no form are a JSON object, or empty', () => {
  assert.deepEqual(parseJsonArgs(''), { args: {} });
  assert.deepEqual(parseJsonArgs('  \n'), { args: {} });
  assert.deepEqual(parseJsonArgs('{"name": "nginx", "n": 2}'), { args: { name: 'nginx', n: 2 } });
  assert.match(parseJsonArgs('{name').problem, /not JSON/);
  assert.match(parseJsonArgs('[1]').problem, /JSON object/);
  assert.match(parseJsonArgs('"x"').problem, /JSON object/);
  assert.match(parseJsonArgs('null').problem, /JSON object/);
});

// The registry.

const schemas = inputSchemas();
const forms = new Map([...schemas].map(([id, schema]) => [id, formFor(schema)]));

test('the registry is read in the order lab/actions.toml lists it, and the lab\'s own two follow', () => {
  assert.deepEqual([...schemas.keys()].slice(0, listedIds().length), listedIds());
  assert.deepEqual([...schemas.keys()].slice(listedIds().length), ['lab.long', 'lab.missing']);
});

test('every action in lab/actions.toml makes a form without the fallback to JSON', () => {
  for (const id of listedIds()) {
    const form = forms.get(id);
    assert.equal(form.ok, true, `${id}: ${form.why}`);
    for (const field of form.fields) {
      assert.ok(field.description !== '', `${id}'s ${field.name} has a hint for the form to show`);
      assert.ok(['input', 'radio', 'select', 'number', 'checkbox'].includes(field.control));
    }
  }
  assert.equal(forms.get('lab.long').ok, true);
  assert.equal(forms.get('lab.missing').ok, true);
});

test('the registry\'s schemas are the ones admind\'s own tests state', () => {
  const name = forms.get('service.restart').fields[0];
  assert.deepEqual([name.name, name.control, name.required, name.minLength, name.maxLength], ['name', 'input', true, 1, 23]);
  assert.equal(name.pattern, '^(?!(check|new|edit|try|keep|undo|secret|admind|authd)$)[A-Za-z0-9_][A-Za-z0-9_.-]*$');
  const certificate = forms.get('certificate.show').fields[0];
  assert.deepEqual([certificate.maxLength, certificate.pattern], [64, '^(?!.*\\.\\.)[a-z0-9][a-z0-9.-]*$']);
  const reach = forms.get('console.reach').fields[0];
  assert.deepEqual([reach.control, reach.words], ['radio', ['private', 'any']]);
  const range = forms.get('metrics.usage').fields[0];
  assert.deepEqual([range.control, range.words], ['radio', ['1h', '24h', '7d', '1y']]);
  const seconds = forms.get('lab.long').fields[0];
  assert.deepEqual([seconds.control, seconds.required, seconds.minimum, seconds.maximum], ['number', false, 1, 600]);
  for (const id of ['service.list', 'certificate.list', 'console.about', 'console.notices', 'checks.last', 'metrics.now', 'lab.missing']) {
    assert.deepEqual(forms.get(id).fields, [], id);
  }
});

test('a service\'s name is held to what admind\'s check holds it to, and the console\'s own are refused where the action refuses them', () => {
  const good = ['a', 'nginx', 'my-app_2', 'a.b', '_x', '0', '1.2.3', 'diskhealth', 'any', 'admind2', 'Admind', 'a'.repeat(23), 'a-'];
  const bad = ['', '.hidden', '-x', '-', '.', 'a b', 'a/b', '../x', 'a@b', 'aé', 'é', 'a\nb', 'nginx\n', '\nnginx', 'a'.repeat(24), 'a'.repeat(256)];
  const own = ['check', 'new', 'edit', 'try', 'keep', 'undo', 'secret'];
  for (const id of ['service.explain', 'service.start', 'service.enable']) {
    for (const name of good) assert.deepEqual(rules(forms.get(id), { name }), [], `${id} takes ${JSON.stringify(name)}`);
    for (const name of [...bad, ...own]) assert.notDeepEqual(rules(forms.get(id), { name }), [], `${id} refuses ${JSON.stringify(name)}`);
    assert.deepEqual(rules(forms.get(id), { name: 'admind' }), [], `${id} takes the console's own service`);
  }
  for (const id of ['service.stop', 'service.restart', 'service.disable']) {
    for (const name of good) assert.deepEqual(rules(forms.get(id), { name }), [], `${id} takes ${JSON.stringify(name)}`);
    for (const name of [...bad, ...own, 'admind', 'authd']) assert.notDeepEqual(rules(forms.get(id), { name }), [], `${id} refuses ${JSON.stringify(name)}`);
  }
  assert.deepEqual(rules(forms.get('service.restart'), { name: 'nginx', force: true }), ['force:unknown']);
  assert.deepEqual(rules(forms.get('service.restart'), {}), ['name:required']);
});

test('a certificate\'s name is held to what admind\'s check holds it to', () => {
  const good = ['this-machine', 'example.org', 'a', '0', 'a-b.c', 'x'.repeat(64)];
  const bad = ['', 'This-Machine', '../etc/passwd', 'x..y', 'a..', '.a', '-a', 'a b', 'aé', 'x'.repeat(65), 'a\n'];
  for (const id of ['certificate.show', 'certificate.renew']) {
    for (const name of good) assert.deepEqual(rules(forms.get(id), { name }), [], `${id} takes ${JSON.stringify(name)}`);
    for (const name of bad) assert.notDeepEqual(rules(forms.get(id), { name }), [], `${id} refuses ${JSON.stringify(name)}`);
  }
});

test('the reach, the usage range and the seconds of a lab action are held to their words and limits', () => {
  assert.deepEqual(rules(forms.get('console.reach'), { reach: 'private' }), []);
  assert.deepEqual(rules(forms.get('console.reach'), { reach: 'any' }), []);
  for (const reach of ['everyone', 'Any', ' any', 'any\n', '']) assert.deepEqual(rules(forms.get('console.reach'), { reach }), ['reach:not-a-word']);
  for (const range of ['1h', '24h', '7d', '1y']) assert.deepEqual(rules(forms.get('metrics.usage'), { range }), []);
  assert.deepEqual(rules(forms.get('metrics.usage'), { range: '2h' }), ['range:not-a-word']);
  assert.deepEqual(rules(forms.get('lab.long'), {}), []);
  assert.deepEqual(rules(forms.get('lab.long'), { seconds: 20 }), []);
  assert.deepEqual(rules(forms.get('lab.long'), { seconds: 0 }), ['seconds:below']);
  assert.deepEqual(rules(forms.get('lab.long'), { seconds: 601 }), ['seconds:above']);
  assert.deepEqual(rules(forms.get('lab.long'), { seconds: '5' }), ['seconds:not-whole']);
});
