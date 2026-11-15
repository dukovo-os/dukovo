// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Steps 2 and 3 of Move to new hardware (design 6.3): unlock the backup
// with its recovery key, and check what changes. The key is a file that
// `backup find` found, chosen by default when it is named after the machine,
// or typed or pasted in a secret field; a typed key wins and clears the
// file's choice, and goes only in the body of `move.plan`, never to the
// Show command panel. What is typed is held by the field alone and is gone
// when Unlock it has taken it. The plan that comes back is drawn as what
// comes across, what differs in the hardware (with the choice of each old
// card's address), what happens after the restart and the command that does
// the same.

import {
  fill, h, icon, uniqueId,
} from '../../dom.js';
import { makeSecretField } from '../../lib/secret-field.js';
import { copyBox } from '../snapshots/copy-box.js';
import {
  afterOf, argsOf, cardLines, comesAcross, hardwareOf, RESTART_COMMAND, showArgs,
} from './model.js';

/** How long the command waits after a choice changes, so that choosing is one call. */
const CHOOSING_MS = 400;

/** What is said under the key field, once. */
const KEY_HINT = 'The key is not kept. After you unlock the backups, Stage it asks for it again unless you chose a key file.';

/**
 * Step 2 in a card. `state` is the page's: `{keys, file, choice}`, the key
 * files, the one chosen and the backup chosen. `hooks.unlockFor(words)`
 * asks an administrator whose console is locked to unlock, and gives
 * whether it is, and `hooks.planned(plan, how)` is called with the plan
 * and `file` or `typed`. Gives `{element, focus, draw}`; `draw()` shows
 * the key files again, and leaves what is typed as it is.
 */
export function makeKeyStep(view, state, hooks) {
  const id = uniqueId('mv-key');
  const heading = h('h2', { id: `${id}-h`, tabindex: '-1' }, 'Unlock it');
  const list = h('fieldset', { class: 'choices' });
  const errors = h('div', { class: 'mv-errors' });
  const field = makeSecretField({
    label: 'Recovery key', kind: 'key', optional: true, hint: KEY_HINT, signal: view.signal,
  });
  const unlock = h('button', { class: 'btn default', type: 'button' }, 'Unlock it');

  /** The key files as radios, the chosen one checked. */
  function draw() {
    const cards = state.keys.map((key) => {
      const input = h('input', { type: 'radio', name: `${id}-file`, value: key.path, checked: key.path === state.file?.path });
      input.addEventListener('change', () => {
        state.file = key;
        field.clear();
        draw();
      });
      return h('label', { class: `radio-card${key.path === state.file?.path ? ' on' : ''}` }, input,
        h('span', { class: 'mv-choice' }, h('b', {}, key.title), h('span', { class: 'small muted mono' }, key.path)));
    });
    fill(list, h('legend', {}, 'A key file on a plugged-in disk'), ...cards);
    list.hidden = cards.length === 0;
  }

  // Typing a key takes the file's choice away: the two are never given together.
  field.element.addEventListener('input', () => {
    if (state.file === null) return;
    state.file = null;
    draw();
  });

  async function run() {
    errors.replaceChildren();
    // Unlocked before the key is taken out of its field, so that a person who turns the dialog down keeps what they typed.
    if (!(await hooks.unlockFor('unlock the backups'))) return;
    const typed = field.take();
    if (typed === '' && state.file === null) {
      field.fail({
        id: null, title: 'Choose a key file or type the recovery key', detail: '', fix: '', retryAfter: null,
      });
      return;
    }
    const how = typed === '' ? 'file' : 'typed';
    const done = await view.api.run('move.plan', argsOf({ source: state.choice.source, file: state.file?.path ?? null, typed }), {
      words: { unlock: 'unlock the backups', running: 'Reading the backups', busy: 'Unlocking…' }, button: unlock, where: { slot: errors },
    });
    if (view.signal.aborted) return;
    if (done.ok) hooks.planned(done.result, how);
    else field.focus();
  }
  unlock.addEventListener('click', run);

  const element = h('section', { class: 'card', 'aria-labelledby': heading.id },
    heading,
    h('p', { class: 'muted' }, 'The recovery key opens the backups. It was shown once when their backup place was added.'),
    list,
    field.element,
    errors,
    h('div', { class: 'btns' }, unlock));
  draw();
  return { element, focus: () => heading.focus(), draw };
}

/** A table of what comes across, `rows` of `comesAcross`. */
function acrossTable(rows) {
  const cells = {};
  const table = h('table', { class: 'tbl' },
    h('caption', { class: 'sr-only' }, 'What comes across'),
    h('tbody', {}, rows.map((row) => {
      const detail = h('small', {}, row.detail);
      cells[row.id] = detail;
      return h('tr', {},
        h('td', {}, h('b', {}, row.title), detail),
        h('td', { class: 'end' }, row.pill ? h('span', { class: `pill ${row.pill.tone}` }, row.pill.words) : h('span', { class: 'small muted' }, row.muted)));
    })));
  return { element: h('div', { class: 'tbl-box' }, table), cells };
}

/**
 * Step 3 in a card. `state` is the page's: `{plan, picks, used,
 * choice}`; `hooks.pick(mac, value)` is called when a card's choice
 * changes, `hooks.next()` for Continue, `links` says which pages this
 * console has (`{storage}`) and `port` is the console's, for the address after
 * the restart. Gives `{element, focus, draw, refresh, focusChoice}`: `draw()` draws the
 * plan, and `refresh()` brings what the choices change up to date in place,
 * so that the choice that was pressed keeps the focus.
 */
export function makeCheck(view, state, hooks, { links = {}, port = '' } = {}) {
  const id = uniqueId('mv-check');
  const body = h('div', { class: 'mv-check' });
  let settings = null;
  let clashes = {};
  let after = null;
  let waiting = null;

  /** The command and the restart's, after the choices stop changing. */
  function drawCommands(box) {
    clearTimeout(waiting);
    waiting = setTimeout(() => {
      const args = showArgs({
        source: state.choice.source, how: state.used.how, file: state.used.path, cards: cardLines(state.plan, state.picks), at: state.plan.at ?? null,
      });
      fill(box, h('h3', {}, 'The same as these commands'), view.api.panel('move.stage', args), copyBox(RESTART_COMMAND, 'The command that restarts this machine'));
    }, CHOOSING_MS);
  }
  view.signal.addEventListener('abort', () => clearTimeout(waiting), { once: true });

  /** What happens after the restart, from the choices. */
  function drawAfter() {
    const found = afterOf(state.plan, state.picks, { port });
    const commands = h('div', { class: 'mv-commands' });
    fill(after,
      h('div', { class: `banner ${found.banner.tone}` }, icon('warn'), h('div', {}, h('p', {}, h('b', {}, found.banner.lead), ' ', found.banner.detail))),
      h('ul', { class: 'mv-after' }, ...found.lines.map((words) => h('li', {}, words))),
      h('p', {}, found.open),
      commands);
    drawCommands(commands);
  }

  /** The row of the hardware's differences, `row` of `hardwareOf`, with its choices. */
  function diffRow(row) {
    const clash = h('p', { class: 'msg', role: 'status' }, row.clash);
    clashes[row.id] = clash;
    const groups = row.groups.map((group) => {
      const name = uniqueId('mv-card');
      return h('fieldset', { class: 'choices' },
        h('legend', { class: row.groups.length > 1 ? '' : 'sr-only' }, group.label),
        ...group.options.map((option) => {
          const input = h('input', {
            type: 'radio', name, value: option.value, checked: option.checked, 'data-card': group.mac,
          });
          input.addEventListener('change', () => hooks.pick(group.mac, option.value));
          return h('label', { class: 'check' }, input, h('span', {}, option.words));
        }));
    });
    return h('li', {},
      h('div', { class: 'mv-diff-head' }, h('b', {}, row.title), h('span', { class: `pill ${row.pill.tone}` }, row.pill.words)),
      ...row.lines.map((line) => h('span', {}, line)),
      ...groups,
      clash,
      ...row.notes.map((note) => h('span', { class: 'small muted' }, note)));
  }

  /** Draws the plan in three cards, and the button that goes on. */
  function draw() {
    clashes = {};
    const { plan, picks } = state;
    const across = acrossTable(comesAcross(plan, picks));
    settings = across.cells.settings;
    const hardware = hardwareOf(plan, picks, links);
    after = h('div', { class: 'mv-after-box' });
    const next = h('button', { class: 'btn default', type: 'button' }, 'Continue');
    next.addEventListener('click', hooks.next);
    fill(body,
      h('section', { class: 'card', 'aria-labelledby': `${id}-across` }, h('h2', { id: `${id}-across`, tabindex: '-1' }, 'What comes across'), across.element),
      h('section', { class: 'card', 'aria-labelledby': `${id}-diff` },
        h('h2', { id: `${id}-diff` }, 'Differences in the hardware'),
        hardware.note && h('p', { class: 'muted' }, hardware.note),
        hardware.rows.length > 0 && h('ul', { class: 'mv-diff' }, ...hardware.rows.map(diffRow))),
      h('section', { class: 'card', 'aria-labelledby': `${id}-after` }, h('h2', { id: `${id}-after` }, 'After the restart'), after),
      h('div', { class: 'btns' }, next));
    drawAfter();
  }

  /** Brings the settings row, the refusal of two cards for one port and the restart up to date with the choices. */
  function refresh() {
    settings.textContent = comesAcross(state.plan, state.picks).find((row) => row.id === 'settings').detail;
    for (const row of hardwareOf(state.plan, state.picks, links).rows) if (clashes[row.id]) clashes[row.id].textContent = row.clash;
    drawAfter();
  }

  /** Puts the focus back on the choice `value` of the card `mac`, after the plan was drawn again. */
  const focusChoice = (mac, value) => [...body.querySelectorAll('input')].find((input) => input.getAttribute('data-card') === mac && input.value === value)?.focus();

  return {
    element: body, focus: () => body.querySelector('h2')?.focus(), draw, refresh, focusChoice,
  };
}
