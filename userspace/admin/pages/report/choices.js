// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// "What went wrong?": the choices as radio cards, the answer or answers under
// them, and the box that makes a report a security problem. The page keeps the
// state; this draws it and says what was done to it. The fields are made once
// and shown or put away, so that what is typed in them stays.

import { fill, h, uniqueId } from '../../dom.js';
import { ANSWER_MAX, noteMax } from './model.js';

/** The three questions of a choice that asks them: the field's name, its words and its hint. */
const QUESTIONS = [
  ['happened', 'What happened?', 'Needed.'],
  ['expected', 'What did you expect?', 'Optional.'],
  ['again', 'How can it be made to happen again?', 'Optional.'],
];
const SECURITY_WORDS = 'Sent encrypted to the security team\'s key: the reports server keeps no readable copy, and nobody else reads it.';

/**
 * Makes the card. `fields` is the page's `{note, happened, expected, again}`,
 * which the card writes as the person types; `onType()` is told of each key and
 * `onField()` of a field left with other words. `onChoose(key)`, `onMore()`,
 * `onSecurity(on)` and `onDiscard()` are what is done to the card.
 */
export function makeChoices({
  fields, onChoose, onMore, onType, onField, onSecurity, onDiscard,
}) {
  const id = uniqueId('report-what');
  const list = h('div', { class: 'rp-choices' });
  const readers = h('p', { class: 'note', hidden: true }, 'A report about a failed start or about a service is made by an administrator.');
  const security = h('input', { type: 'checkbox' });
  const discard = h('button', { class: 'btn link', type: 'button', hidden: true, onclick: onDiscard }, 'Discard');
  let drawn = null;

  /** A field: its label, its hint and a textarea that writes `fields[name]`. */
  function field(name, label, hint) {
    const input = h('textarea', {
      id: `${id}-${name}`, class: 'input', rows: '3', maxlength: String(ANSWER_MAX), 'aria-describedby': hint ? `${id}-${name}-hint` : null,
    });
    input.addEventListener('input', () => {
      fields[name] = input.value;
      onType();
    });
    input.addEventListener('change', onField);
    return { name, input, node: h('div', { class: 'field' }, h('label', { for: input.id }, label), hint && h('span', { id: `${id}-${name}-hint`, class: 'hint' }, hint), input) };
  }

  const note = field('note', 'Anything to add? (optional)', '');
  const asked = QUESTIONS.map(([name, label, hint]) => field(name, label, hint));
  const noteBox = h('div', { class: 'rp-answers', hidden: true }, note.node);
  const questions = h('div', { class: 'rp-answers', hidden: true }, ...asked.map((one) => one.node));
  security.addEventListener('change', () => onSecurity(security.checked));

  const element = h('section', { class: 'card', 'aria-labelledby': `${id}-h` },
    h('h2', { id: `${id}-h` }, 'What went wrong?'),
    h('fieldset', { class: 'choices', 'aria-labelledby': `${id}-h` }, list),
    readers,
    noteBox,
    questions,
    h('label', { class: 'check' }, security, h('span', {}, h('b', {}, 'This is a security problem.'), ' ', SECURITY_WORDS)),
    h('div', { class: 'btns' }, discard));

  /** The radio card of `choice`; `on` says that it is the chosen one. */
  function card(choice, on) {
    const input = h('input', { type: 'radio', name: id, value: choice.key, checked: on });
    input.addEventListener('change', () => {
      if (input.checked) onChoose(choice.key);
    });
    return h('label', { class: `radio-card${on ? ' on' : ''}` }, input,
      h('span', {}, h('b', {}, choice.title), h('br'), h('span', { class: 'muted' }, choice.words)));
  }

  return {
    element,

    /** Puts what `fields` holds in the textareas, after the page changed it. */
    setValues() {
      for (const one of [note, ...asked]) one.input.value = fields[one.name];
    },

    /**
     * Draws the card: `shown` the choices and `more` the crashes behind Show
     * more, `key` the chosen one and `choice` its object, `security` the box,
     * `readerNote` whether a reader is told of what is left out, and `dirty`
     * whether there is anything to discard.
     */
    paint({
      shown, more, key, choice, security: on, readerNote, dirty,
    }) {
      if (drawn?.shown !== shown || drawn?.more !== more) {
        fill(list, shown.map((one) => card(one, one.key === key)), more > 0 && h('button', { class: 'btn link', type: 'button', onclick: onMore }, `Show ${more} more`));
      } else {
        for (const label of list.querySelectorAll('.radio-card')) {
          const input = label.querySelector('input');
          input.checked = input.getAttribute('value') === key;
          label.classList.toggle('on', input.checked);
        }
      }
      drawn = { shown, more };
      readers.hidden = !readerNote;
      noteBox.hidden = !choice || choice.questions;
      questions.hidden = !choice || !choice.questions;
      if (choice) note.input.setAttribute('maxlength', String(noteMax(choice)));
      security.checked = on;
      discard.hidden = !dirty;
    },
  };
}
