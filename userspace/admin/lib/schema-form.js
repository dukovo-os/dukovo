// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The form for an action's arguments (design 11.5), drawn from what
// lib/schema-model.js makes of its schema: an input for a string, radio
// buttons for four words or fewer and a menu for more, a number input, a
// checkbox. A hint under each control says what its schema says, tied to
// it with `aria-describedby`; a field that is needed says "(needed)" in its
// label. A schema outside the subset gives one text area for JSON and a
// note, never a crash.
//
// A field is checked as the person leaves it, and all of them when the form
// is asked to (`show`). The words of a problem sit under the field, in a
// region that is there from the start so that a screen reader says what is
// put in it, and the first field that has one takes focus.

import { afterPress, h, icon, uniqueId } from '../dom.js';
import {
  argsFrom, checkArgs, JSON_NOTE, labelWords, parseJsonArgs, ruleBroken, startingValues, wordsFor,
} from './schema-model.js';

/** The words for an action that has nothing to fill in. */
export const NO_FIELDS = 'This action takes no arguments. Nothing to fill in.';

/** The words of a problem, with the mark that says it is one. */
const problemWords = (words) => [icon('warn'), h('span', {}, words)];

/**
 * One field of a form: its `element`, its `control` (the first input, which
 * takes focus), `value()` as the controls hold it, a `problem` region that
 * `say(words)` fills and empties, and the `inputs` that change events come
 * from.
 */
function fieldFor(field, start, id) {
  const hint = field.description ? h('p', { id: `${id}-hint`, class: 'hint' }, field.description) : null;
  const problem = h('div', { id: `${id}-error`, class: 'msg', role: 'alert' });
  const describedBy = [hint && `${id}-hint`, `${id}-error`].filter(Boolean).join(' ');
  let inputs;
  let element;
  let value;
  if (field.control === 'radio') {
    const radios = field.words.map((word, at) => h('input', {
      type: 'radio', id: `${id}-${at}`, name: id, value: word, checked: word === start,
    }));
    const cards = radios.map((radio, at) => {
      const card = h('label', { class: 'radio-card', for: radio.id }, radio, h('span', {}, field.words[at]));
      card.classList.toggle('on', radio.checked);
      return card;
    });
    for (const radio of radios) {
      radio.addEventListener('change', () => cards.forEach((card, at) => card.classList.toggle('on', radios[at].checked)));
    }
    inputs = radios;
    value = () => radios.find((radio) => radio.checked)?.value ?? '';
    element = h('div', { class: 'field' },
      h('fieldset', { class: 'choices', 'aria-describedby': describedBy },
        h('legend', {}, labelWords(field)),
        hint,
        h('div', { class: 'cards-row' }, ...cards)),
      problem);
  } else if (field.control === 'checkbox') {
    const box = h('input', { id, type: 'checkbox', checked: start === true, 'aria-describedby': describedBy });
    inputs = [box];
    value = () => box.checked;
    element = h('div', { class: 'field' }, h('label', { class: 'check', for: id }, box, h('span', {}, labelWords(field))), hint, problem);
  } else if (field.control === 'select') {
    const menu = h('select', { id, class: 'input', 'aria-describedby': describedBy },
      h('option', { value: '' }, 'Choose one'),
      ...field.words.map((word) => h('option', { value: word, selected: word === start }, word)));
    inputs = [menu];
    value = () => menu.value;
    element = h('div', { class: 'field' }, h('label', { for: id }, labelWords(field)), menu, hint, problem);
  } else {
    const number = field.control === 'number';
    const input = h('input', {
      id,
      class: 'input',
      type: number ? 'number' : 'text',
      value: start,
      maxlength: field.maxLength,
      min: number ? field.minimum : null,
      max: number ? field.maximum : null,
      step: number ? '1' : null,
      inputmode: number ? 'numeric' : null,
      autocomplete: 'off',
      autocapitalize: 'off',
      spellcheck: 'false',
      'aria-describedby': describedBy,
    });
    inputs = [input];
    value = () => input.value;
    element = h('div', { class: 'field' }, h('label', { for: id }, labelWords(field)), input, hint, problem);
  }
  return {
    element,
    control: inputs.find((input) => input.checked) ?? inputs[0],
    inputs,
    value,
    say(words) {
      problem.replaceChildren(...(words ? problemWords(words) : []));
      for (const input of inputs) {
        input.classList.toggle('bad', Boolean(words));
        if (words) input.setAttribute('aria-invalid', 'true');
        else input.removeAttribute('aria-invalid');
      }
    },
  };
}

/** The text area that stands in for a form when the schema is outside the subset. */
function jsonField(start, id) {
  const area = h('textarea', {
    id, class: 'input mono', rows: '6', spellcheck: 'false', autocomplete: 'off', autocapitalize: 'off', 'aria-describedby': `${id}-hint ${id}-error`,
  }, start);
  const problem = h('div', { id: `${id}-error`, class: 'msg', role: 'alert' });
  return {
    element: h('div', { class: 'field' },
      h('label', { for: id }, 'Arguments, as JSON'),
      area,
      h('p', { id: `${id}-hint`, class: 'hint' }, JSON_NOTE),
      problem),
    control: area,
    inputs: [area],
    value: () => area.value,
    say(words) {
      problem.replaceChildren(...(words ? problemWords(words) : []));
      area.classList.toggle('bad', Boolean(words));
      if (words) area.setAttribute('aria-invalid', 'true');
      else area.removeAttribute('aria-invalid');
    },
  };
}

/**
 * The form for `result`, which `formFor` gave (`{ok, fields}` or `{ok:
 * false, why}`), starting from `args`, the arguments the thing that was
 * found already gives. `onChange()` is called when a control changes.
 * Gives:
 *
 * - `element`: the controls, to put in a `<form>`;
 * - `fields`: each field's first input by the argument's name, for
 *   lib/problem.js to put admind's own words under; `input` is the first;
 * - `read()`: `{args, problems}` from what the controls hold, with no
 *   change to the page; for the text area of JSON, `args` is null when the
 *   text is not a JSON object;
 * - `show(problems)`: the words of each problem under its field, the
 *   others emptied, and focus on the first;
 * - `focusFirst()`.
 */
export function schemaForm(result, { args = {}, onChange = () => {} } = {}) {
  const base = uniqueId('arg');
  if (!result.ok) {
    const only = jsonField(Object.keys(args).length > 0 ? JSON.stringify(args, null, 2) : '', base);
    only.inputs[0].addEventListener('input', () => {
      only.say('');
      onChange();
    });
    return {
      element: h('div', { class: 'schema-form' }, only.element),
      fields: {},
      input: only.control,
      fallback: true,
      read() {
        const parsed = parseJsonArgs(only.value());
        return parsed.problem
          ? { args: null, problems: [{ field: '', rule: 'json', words: parsed.problem }] }
          : { args: parsed.args, problems: [] };
      },
      show(problems) {
        only.say(problems[0]?.words ?? '');
        if (problems.length > 0) only.control.focus();
      },
      focusFirst: () => only.control.focus(),
    };
  }

  const form = result;
  const start = startingValues(form, args);
  const made = new Map(form.fields.map((field, at) => [field.name, fieldFor(field, start[field.name], `${base}-${at}`)]));
  const read = () => {
    const values = Object.fromEntries([...made].map(([name, one]) => [name, one.value()]));
    const given = argsFrom(form, values);
    return { args: given, problems: checkArgs(form, given) };
  };
  for (const [name, one] of made) {
    const field = form.fields.find((candidate) => candidate.name === name);
    for (const input of one.inputs) {
      // A word that is wrong goes when the person starts to put it right.
      input.addEventListener('input', () => {
        one.say('');
        onChange();
      });
      input.addEventListener('change', onChange);
      // As the person leaves a text or number field, it is checked alone,
      // after the press that left it, which the words would otherwise move
      // the button under.
      if (field.control === 'input' || field.control === 'number') {
        input.addEventListener('blur', () => afterPress(() => {
          const held = read().args[name];
          const rule = held === undefined ? null : ruleBroken(field, held);
          one.say(rule ? wordsFor(field, rule) : '');
        }));
      }
    }
  }
  return {
    element: h('div', { class: 'schema-form' },
      form.description && h('p', { class: 'hint' }, form.description),
      made.size === 0 ? h('p', { class: 'muted' }, NO_FIELDS) : [...made.values()].map((one) => one.element)),
    fields: Object.fromEntries([...made].map(([name, one]) => [name, one.control])),
    input: [...made.values()][0]?.control ?? null,
    fallback: false,
    read,
    show(problems) {
      const byField = new Map(problems.map((problem) => [problem.field, problem.words]));
      for (const [name, one] of made) one.say(byField.get(name) ?? '');
      const first = problems.map((problem) => made.get(problem.field)).find(Boolean);
      first?.control.focus();
    },
    focusFirst: () => [...made.values()][0]?.control.focus(),
  };
}
