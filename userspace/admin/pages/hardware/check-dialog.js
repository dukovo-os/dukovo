// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The dialog of Check the hardware: what a quick check does, the areas as
// choices (all on), and the full check that takes hours. Check now starts
// the task through the page (`ctx.startCheck`). The page reads this file
// when the button is first pressed.

import { h, icon, uniqueId } from '../../dom.js';
import { openFormDialog } from '../../lib/form-dialog.js';
import {
  AREAS, checkArgs, FULL_WORDS, NO_AREA, QUICK_WORDS,
} from './check-model.js';

/** Opens the dialog and resolves, when it closes, with whether a check was started. */
export function openCheckDialog(ctx) {
  const id = uniqueId('hw-check');
  const boxes = AREAS.map((area) => h('input', { type: 'checkbox', id: `${id}-${area.id}` }));
  for (const box of boxes) box.checked = true;
  const full = h('input', { type: 'checkbox', id: `${id}-full`, 'aria-describedby': `${id}-full-words` });
  full.checked = false;
  const fullWords = h('p', { id: `${id}-full-words`, class: 'hint' }, FULL_WORDS);
  fullWords.hidden = true;
  full.addEventListener('change', () => {
    fullWords.hidden = !full.checked;
  });

  const argsNow = () => checkArgs(AREAS.filter((_, at) => boxes[at].checked).map((area) => area.id), full.checked === true);
  const fields = [
    h('fieldset', { class: 'choices' },
      h('legend', {}, 'What to check'),
      AREAS.map((area, at) => h('label', { class: 'check', for: boxes[at].id }, boxes[at], h('span', {}, area.label)))),
    h('div', { class: 'field' },
      h('label', { class: 'check', for: full.id }, full, h('span', {}, 'A full check, which takes hours')),
      fullWords),
  ];
  return openFormDialog(ctx.view, {
    title: 'Check the hardware',
    icon: 'chip',
    lead: QUICK_WORDS,
    fields,
    label: 'Check now',
    command: () => {
      const args = argsNow();
      return args && { action: 'hardware.check', args };
    },
    async submit(button, slot) {
      const args = argsNow();
      if (args === null) {
        slot.append(h('p', { class: 'msg', role: 'alert' }, icon('warn'), h('span', {}, NO_AREA)));
        return false;
      }
      return ctx.startCheck(args, button, slot);
    },
  });
}
