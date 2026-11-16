// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// "What the report holds": the parts of the report and their sizes, what was
// taken out of it before anything is shown, and the box that leaves the logs
// out. Drawn when there is a text to hold.

import { fill, h, icon, uniqueId } from '../../dom.js';
import {
  holdsWords, partsOf, takenOutWords,
} from './model.js';

/**
 * Makes the card. `onLogs(left)` is called when the box that leaves the logs
 * out is changed.
 */
export function makeHolds({ onLogs }) {
  const id = uniqueId('report-holds');
  const size = h('span', { class: 'more muted' });
  const parts = h('ul', { class: 'rp-parts' });
  const taken = h('p', {});
  const logs = h('input', { type: 'checkbox', id: `${id}-logs` });
  const logsBox = h('label', { class: 'check', hidden: true }, logs, h('span', {}, h('b', {}, 'Leave out the logs'), ' ', 'The report is drafted again without them.'));
  logs.addEventListener('change', () => onLogs(logs.checked));
  const element = h('section', { class: 'card', 'aria-labelledby': `${id}-h`, hidden: true },
    h('h2', { id: `${id}-h` }, 'What the report holds', size),
    parts,
    h('div', { class: 'banner info' }, icon('info'), h('div', {}, taken)),
    logsBox);

  return {
    element,

    /** Draws the card for `answer`, the text reportd answered, or hides it when there is none; `withoutLogs` is the box. */
    paint(answer, { withoutLogs, drafting }) {
      element.hidden = !answer;
      if (!answer) return;
      size.textContent = holdsWords(answer);
      fill(parts, partsOf(answer).map((part) => h('li', {}, part.title, ' ', h('span', { class: 'muted' }, `· ${part.size}`))));
      taken.textContent = takenOutWords(answer.cut_by);
      logs.checked = withoutLogs;
      logs.disabled = drafting;
      logsBox.hidden = !(answer.has_logs || withoutLogs);
    },
  };
}
