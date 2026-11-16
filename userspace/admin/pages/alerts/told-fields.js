// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The fields that say what a channel is told: the kinds of alert, and how bad
// an alert has to be and whether a clearing is told. The email form, the
// webhook dialog, "This browser" and the dialog that edits a channel use them.

import { h, uniqueId } from '../../dom.js';
import { KINDS, kindsOf } from './channels-model.js';

/** What each kind holds, for the checkboxes of the form. */
export const HOLDS = Object.freeze({
  data: 'A disk failing or wearing out, a filesystem nearly full, a mirror missing a disk, a backup failed or late.',
  stopped: 'A service, an update or a start that failed, a certificate running out, the kernel stopping, memory running low.',
  hardware: 'A sensor or a disk running hot, the memory test at the start.',
});

/** What is said of the alerts that fit no kind, and of a test. */
export const UNTAGGED_WORDS = 'Alerts that fit none of these, and test alerts, go to every channel.';

/**
 * The checkboxes of the three kinds, checked as `channel` (as alertd lists it)
 * is told of them, all of them for none. Gives `{element, ids(), problem()}`:
 * the ids that are checked, and the words of what is wrong, or null.
 */
export function kindsFields(channel = null) {
  const told = kindsOf(channel);
  const id = uniqueId('kinds');
  const boxes = KINDS.map((kind) => {
    const input = h('input', { type: 'checkbox', checked: told.includes(kind.id), 'aria-describedby': `${id}-${kind.id}` });
    const element = h('label', { class: 'check' }, input,
      h('span', {}, h('b', {}, kind.label), ' ', h('span', { id: `${id}-${kind.id}`, class: 'hint' }, HOLDS[kind.id])));
    return { id: kind.id, input, element };
  });
  const ids = () => boxes.filter((box) => box.input.checked).map((box) => box.id);
  return {
    element: h('div', { class: 'field', role: 'group', 'aria-labelledby': id },
      h('span', { id, class: 'label' }, 'What it is told of'),
      ...boxes.map((box) => box.element),
      h('span', { class: 'hint' }, UNTAGGED_WORDS)),
    ids,
    problem: () => (ids().length === 0 ? 'Choose at least one kind of alert.' : null),
  };
}

/**
 * How bad an alert has to be, and whether a clearing is told, as `channel`
 * has them (every alert, and when one clears, for none). Gives `{element,
 * values()}`, `values()` being `{severity, cleared}`.
 */
export function severityFields(channel = null) {
  const id = uniqueId('severity');
  const one = (value, words) => {
    const input = h('input', {
      type: 'radio', name: id, value, checked: (channel?.severity === 'critical' ? 'critical' : 'warning') === value,
    });
    return { input, element: h('label', { class: 'check' }, input, h('span', {}, words)) };
  };
  const every = one('warning', 'Every alert');
  const critical = one('critical', 'Critical alerts only');
  const cleared = h('input', { type: 'checkbox', checked: channel?.cleared !== false });
  return {
    element: h('div', { class: 'field', role: 'group', 'aria-labelledby': id },
      h('span', { id, class: 'label' }, 'How bad'),
      every.element,
      critical.element,
      h('label', { class: 'check' }, cleared, h('span', {}, 'Tell me when an alert clears'))),
    values: () => ({ severity: critical.input.checked ? 'critical' : 'warning', cleared: cleared.checked }),
  };
}
