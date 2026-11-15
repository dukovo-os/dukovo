// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The buttons of the page of one service (design 4.4): Run now, Stop and
// Restart, and the switch Starts at boot. Each runs the console's action for
// it the way every page runs one (lib/action.js: a reader is told without a
// request, a locked administrator is asked to unlock, the command shows under
// the button while Show commands is on). The console's own services are not
// stopped from the page, and it says why before the person tries.

import { h, uniqueId } from '../../dom.js';
import { copyBox } from './cards.js';
import { BOOT_HINT, controlsOf, wordsFor } from './detail-model.js';

/**
 * The controls of the service `name`. `done()` is called after an action ran,
 * so that the page reads the service again. `element` is what to place;
 * `set(service, admin)` draws it for the service as it is now. It is drawn
 * again only when what it offers changes, and never while an action runs, so
 * that a button that was pressed is not replaced under the person's hand.
 */
export function makeControls(view, name, done) {
  const element = h('div', { class: 'svc-controls' });
  let last = null;
  let running = false;
  let waiting = null;

  /** Runs `action` for the service, with the button `button` busy while it does. */
  async function press(action, button) {
    running = true;
    view.slot.replaceChildren();
    let result = { ok: false };
    try {
      result = await view.api.run(action, { name }, { words: wordsFor(action, name), button });
    } catch (error) {
      view.problem(error);
    } finally {
      running = false;
    }
    if (waiting) {
      const [service, admin] = waiting;
      waiting = null;
      set(service, admin);
    }
    if (result.ok) done();
  }

  /** A button of `controls.buttons`, with the command it runs under it. */
  const buttonOf = (entry) => {
    const button = h('button', { class: `btn${entry.id === 'start' ? ' default' : ''}`, type: 'button', 'data-action': entry.id }, entry.label);
    button.addEventListener('click', () => press(entry.action, button));
    return h('div', { class: 'svc-action' }, button, view.api.inline(entry.action, { name }));
  };

  /** The switch Starts at boot, and what turning it on or off also does. */
  const switchOf = (boot, noteId) => {
    const button = h('button', {
      class: 'svc-boot', type: 'button', role: 'switch', 'aria-checked': String(boot.on), 'aria-describedby': boot.fixed ? noteId : null,
      'aria-disabled': boot.fixed ? 'true' : null, 'data-action': 'boot',
    }, h('span', { class: `sw${boot.on ? ' on' : ''}`, 'aria-hidden': 'true' }), 'Starts at boot');
    if (!boot.fixed) button.addEventListener('click', () => press(boot.action, button));
    return h('div', { class: 'svc-action svc-switch' }, button, h('p', { class: 'hint' }, BOOT_HINT), !boot.fixed && view.api.inline(boot.action, { name }));
  };

  function set(service, admin) {
    if (running) {
      waiting = [service, admin];
      return;
    }
    const controls = controlsOf(service, admin);
    const key = JSON.stringify(controls);
    if (key === last) return;
    last = key;
    const held = element.contains(document.activeElement) ? document.activeElement.dataset.action : null;
    const noteId = uniqueId('svc-note');
    element.replaceChildren(
      ...(controls.buttons.length > 0 ? [h('div', { class: 'btns svc-actions' }, controls.buttons.map(buttonOf))] : []),
      ...(controls.boot ? [switchOf(controls.boot, noteId)] : []),
      ...(controls.note ? [h('div', { class: 'svc-note' },
        h('p', { id: noteId, class: controls.boot ? 'muted' : 'note' }, controls.note.words),
        controls.note.command && copyBox(controls.note.command))] : []));
    if (held) {
      const same = element.querySelector(`[data-action="${held}"]`) ?? element.querySelector('button');
      same?.focus();
    }
  }

  return { element, set };
}
