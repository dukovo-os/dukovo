// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The form that adds, replaces or removes a rule (design 10.3): ready-made
// services as chips, allow or deny, the protocol, a port or range, where it
// comes from and by which card, and a window that is undone by itself unless
// it is kept. Before a change is made, the rules after it are checked against
// the browser's own connection (rules-model.js): a change that would stop it
// says so, still offers Try, and does not offer Apply without trying.

import {
  clearError, errorSlot, focusBack, h, icon,
} from '../../dom.js';
import { WINDOWS } from '../../lib/trial-model.js';
import {
  actionOf, blankForm, CHIPS, connectionAfter, formOfRule, parseRule, PROTOCOLS, ruleOfForm, ruleText, wordsOf,
} from './rules-model.js';

/** How long after the last key the command shown under the form is made again. */
const COMMAND_MS = 300;

/** The window the form starts with, in seconds. */
const DEFAULT_SECONDS = 60;

/** The address of the page without the form's place in it. */
const closedAddress = () => `${location.pathname}${location.search}`;

/** A radio card: a labelled radio button in the box the console draws for them. */
function radioCard(name, value, title) {
  const input = h('input', { type: 'radio', name, value });
  return { input, label: h('label', { class: 'radio-card' }, input, h('span', {}, h('b', {}, title))) };
}

/** Puts `why` under `input`, in `slot`, and marks the input. */
function showWhy(slot, input, why) {
  slot.replaceChildren(icon('warn'), h('span', {}, why));
  input.classList.add('bad');
  input.setAttribute('aria-invalid', 'true');
  input.setAttribute('aria-describedby', slot.id);
}

/**
 * The form. `banner` is the trial banner and `reread()` reads the rules again.
 * Gives `{element, open(mode), close(), set(context)}`: `mode` is `{kind:
 * 'add', words}` (`words` a rule to start from, or none), `{kind: 'replace',
 * number}` or `{kind: 'remove', number}`; `context` is `{rules, cards, here,
 * trial}`: the rules as rules, the names of the cards, the network of the
 * card the browser comes through as text, and whether a trial runs.
 */
export function makeAdd(view, { banner, reread }) {
  let context = { rules: [], cards: [], here: null, trial: false };
  let mode = null;
  let form = blankForm();
  let seconds = DEFAULT_SECONDS;
  let opener = null;
  let commandTimer = null;

  const heading = h('h2', { id: 'fw-add-h', tabindex: '-1' });
  const lead = h('p', { class: 'note', role: 'status' });

  // ---- the fields -----------------------------------------------------------------

  const chips = h('div', { class: 'fw-chips', role: 'group', 'aria-label': 'Ready-made services' },
    CHIPS.map((chip) => {
      const button = h('button', { class: 'btn small fw-chip', type: 'button', 'data-chip': chip.id }, `${chip.label} ${chip.ports}`);
      button.addEventListener('click', () => {
        form.protocol = chip.protocol;
        form.ports = chip.ports;
        showForm();
        changed();
        ports.focus();
      });
      return button;
    }));
  const allow = radioCard('fw-action', 'allow', 'Allow');
  const deny = radioCard('fw-action', 'deny', 'Deny');
  const action = h('fieldset', { class: 'choices fw-action' }, h('legend', {}, 'What to do'), h('div', { class: 'cards-row' }, allow.label, deny.label));
  const protocol = h('select', { id: 'fw-protocol', class: 'input' }, PROTOCOLS.map((one) => h('option', { value: one.id }, one.label)));
  const ports = h('input', { id: 'fw-ports', class: 'input mono', type: 'text', autocomplete: 'off', spellcheck: 'false' });
  const portsError = errorSlot('fw-ports-error');
  const portsField = h('div', { class: 'field' },
    h('label', { for: ports.id }, 'Port or range of ports'), ports,
    h('span', { class: 'hint', id: 'fw-ports-hint' }, 'One port, such as 22, or a range, such as 8000-8100.'), portsError);
  ports.setAttribute('aria-describedby', 'fw-ports-hint');
  const anywhere = radioCard('fw-from', 'anywhere', 'Anywhere');
  const network = radioCard('fw-from', 'network', 'This network');
  const address = radioCard('fw-from', 'address', 'An address or network…');
  const from = h('input', { id: 'fw-from-text', class: 'input mono', type: 'text', autocomplete: 'off', spellcheck: 'false', 'aria-label': 'Address or network' });
  const fromError = errorSlot('fw-from-error');
  const fromField = h('fieldset', { class: 'choices fw-from' },
    h('legend', {}, 'From'), h('div', { class: 'cards-row' }, anywhere.label, network.label, address.label),
    h('div', { class: 'field' }, from, h('span', { class: 'hint', id: 'fw-from-hint' }, 'An address such as 203.0.113.9, or a network such as 192.168.1.0/24.'), fromError));
  from.setAttribute('aria-describedby', 'fw-from-hint');
  const on = h('select', { id: 'fw-on', class: 'input' });
  const onField = h('div', { class: 'field' }, h('label', { for: on.id }, 'On'), on);
  const windowSelect = h('select', { id: 'fw-window', class: 'input' }, WINDOWS.map((one) => h('option', { value: String(one.seconds) }, one.label)));
  const windowField = h('div', { class: 'field' },
    h('label', { for: windowSelect.id }, 'Undo by itself unless kept within'), windowSelect,
    h('span', { class: 'hint' }, 'The rules go back as they were if you do not keep the change in time.'));
  const fields = h('div', { class: 'fw-fields' }, chips, action, h('div', { class: 'fw-two' }, h('div', { class: 'field' }, h('label', { for: protocol.id }, 'Protocol'), protocol), portsField), fromField, onField);

  // ---- what the form says --------------------------------------------------------

  const warning = h('div', { class: 'fw-warning' });
  const runs = h('div', { class: 'fw-runs', hidden: true });
  const problems = h('div', { class: 'fw-problems' });
  const tryButton = h('button', { class: 'btn default', type: 'submit' });
  const applyButton = h('button', { class: 'btn', type: 'button' }, 'Apply without trying');
  const cancelButton = h('button', { class: 'btn', type: 'button' }, 'Cancel');
  const confirm = h('div', { class: 'banner warn fw-confirm', role: 'alert', hidden: true });
  const element = h('form', { class: 'card fw-add', id: 'add', 'aria-labelledby': heading.id, hidden: true, novalidate: true },
    heading, lead, fields, windowField, warning, runs, confirm, problems,
    h('div', { class: 'btns' }, tryButton, applyButton, cancelButton));

  /** The rule being replaced or removed, as the rules have it now, or null. */
  const oldRule = () => (mode?.kind === 'add' ? null : context.rules[mode.number - 1] ?? null);

  /** The change the form makes: `{change}`, or `{field, why}` for a value that cannot work. */
  function changeOf() {
    if (mode.kind === 'remove') return oldRule() === null ? { why: 'That rule is not there any more.', field: null } : { change: { kind: 'remove', old: oldRule() } };
    const made = ruleOfForm(form, context.here);
    if (!made.rule) return made;
    if (mode.kind === 'add') return { change: { kind: 'add', rule: made.rule } };
    return oldRule() === null ? { why: 'That rule is not there any more.', field: null } : { change: { kind: 'replace', old: oldRule(), rule: made.rule } };
  }

  /** Puts the values of the form's model into the fields. */
  function showForm() {
    allow.input.checked = form.action === 'allow';
    deny.input.checked = form.action === 'deny';
    protocol.value = form.protocol;
    ports.value = form.ports;
    anywhere.input.checked = form.from.kind === 'anywhere';
    network.input.checked = form.from.kind === 'network';
    address.input.checked = form.from.kind === 'address';
    from.value = form.from.text;
    on.value = form.on;
    mark();
  }

  /** Shows which radio cards are chosen, the fields a protocol has and the text of From when it is wanted. */
  function mark() {
    for (const one of [allow, deny, anywhere, network, address]) one.label.classList.toggle('on', one.input.checked);
    const ping = form.protocol === 'ping';
    portsField.hidden = ping;
    from.hidden = form.from.kind !== 'address';
    network.label.hidden = context.here === null;
  }

  /** Takes the sentences away. */
  function clearAll() {
    for (const [slot, input] of [[portsError, ports], [fromError, from]]) {
      clearError(slot, input);
      if (input === ports) input.setAttribute('aria-describedby', 'fw-ports-hint');
      if (input === from) input.setAttribute('aria-describedby', 'fw-from-hint');
    }
    problems.replaceChildren();
  }

  /** Checks the form; says what is wrong under its field and moves to it. Gives the change, or null. */
  function check() {
    clearAll();
    const made = changeOf();
    if (made.change) return made.change;
    const place = { ports: [portsError, ports], from: [fromError, from] }[made.field];
    if (place) {
      showWhy(place[0], place[1], made.why);
      place[1].focus();
    } else {
      problems.replaceChildren(h('p', { class: 'msg', role: 'alert' }, icon('warn'), h('span', {}, made.why)));
    }
    return null;
  }

  /** Draws what depends on the values: the label of Try, the warning of a cut, Apply without trying and the command. */
  function drawState() {
    const label = WINDOWS.find((one) => one.seconds === seconds)?.label ?? `${seconds} seconds`;
    tryButton.textContent = `Try for ${label}`;
    const made = changeOf();
    const found = made.change ? connectionAfter(context.rules, made.change, view.session) : { state: 'unknown' };
    // The warning is an alert when it first appears, and is not read again each time the values change.
    if (found.state === 'denied') {
      if (warning.dataset.words !== found.words) {
        warning.dataset.words = found.words;
        warning.replaceChildren(h('div', { class: 'banner bad', role: 'alert' }, icon('warn'),
          h('div', {}, h('p', { id: 'fw-cut' }, found.words), h('p', { class: 'small' }, 'Try for a time, and the rules go back by themselves if you cannot keep the change.'))));
      }
      tryButton.setAttribute('aria-describedby', 'fw-cut');
    } else {
      warning.replaceChildren();
      delete warning.dataset.words;
      tryButton.removeAttribute('aria-describedby');
    }
    // Apply without trying only when the browser's connection is found to be let in.
    applyButton.hidden = found.state !== 'allowed';
    if (applyButton.hidden) confirm.hidden = true;
    drawCommand(made);
  }

  /** The command, made 300 ms after the last change, while Show commands is on. */
  function drawCommand(made) {
    runs.hidden = view.prefs?.commands !== 'shown';
    clearTimeout(commandTimer);
    if (runs.hidden) return;
    commandTimer = setTimeout(() => {
      const now = made ?? changeOf();
      if (!now.change) {
        runs.replaceChildren(h('h3', {}, 'This runs'), h('p', { class: 'muted' }, 'The command shows when every field is right.'));
        return;
      }
      const { action: id, args } = actionOf(now.change, seconds);
      runs.replaceChildren(h('h3', {}, 'This runs'), view.api.panel(id, args));
    }, COMMAND_MS);
  }
  view.onPrefs(() => drawCommand());
  view.signal.addEventListener('abort', () => clearTimeout(commandTimer), { once: true });

  /** A field changed: its sentence goes, and what depends on the values is drawn again. */
  function changed(input, slot) {
    if (input) {
      clearError(slot, input);
      input.setAttribute('aria-describedby', input === ports ? 'fw-ports-hint' : 'fw-from-hint');
    }
    confirm.hidden = true;
    mark();
    drawState();
  }

  for (const [one, value] of [[allow, 'allow'], [deny, 'deny']]) {
    one.input.addEventListener('change', () => {
      form.action = value;
      changed();
    });
  }
  for (const [one, kind] of [[anywhere, 'anywhere'], [network, 'network'], [address, 'address']]) {
    one.input.addEventListener('change', () => {
      form.from = { kind, text: kind === 'address' ? from.value : '' };
      changed();
      if (kind === 'address') from.focus();
    });
  }
  protocol.addEventListener('change', () => {
    form.protocol = protocol.value;
    changed();
  });
  ports.addEventListener('input', () => {
    form.ports = ports.value;
    changed(ports, portsError);
  });
  from.addEventListener('input', () => {
    form.from = { kind: 'address', text: from.value };
    changed(from, fromError);
  });
  on.addEventListener('change', () => {
    form.on = on.value;
    changed();
  });
  windowSelect.addEventListener('change', () => {
    seconds = Number(windowSelect.value);
    drawState();
  });

  // ---- making the change -----------------------------------------------------------

  const serverFields = () => ({
    ports, new_ports: ports, from, new_from: from, on, new_on: on,
  });

  /** Runs the change with `tried` seconds of a trial or none; gives whether it was made. */
  async function run(change, tried, button) {
    const { action: id, args } = actionOf(change, tried);
    const verb = { add: 'add a rule', replace: 'replace a rule', remove: 'remove a rule' }[change.kind];
    const result = await view.api.run(id, args, {
      button,
      words: {
        unlock: verb,
        running: tried === null ? 'Changing the rules' : 'Trying the rules',
        done: tried === null ? 'The rules are changed.' : 'The rules are changed for a trial.',
        busy: tried === null ? 'Applying…' : 'Trying…',
      },
      where: { slot: problems, fields: serverFields() },
    });
    if (!result.ok) return false;
    close(false);
    if (tried !== null) banner.begin({ seconds: tried });
    reread();
    return true;
  }

  /** Holds the buttons while `work` runs, and leaves them off when a trial now runs. */
  async function guarded(work) {
    for (const one of [tryButton, applyButton, cancelButton]) one.disabled = true;
    try {
      return await work();
    } finally {
      for (const one of [tryButton, applyButton, cancelButton]) one.disabled = false;
    }
  }

  element.addEventListener('submit', async (event) => {
    event.preventDefault();
    if (context.trial) return;
    const change = check();
    if (change) await guarded(() => run(change, seconds, tryButton));
  });

  applyButton.addEventListener('click', () => {
    if (context.trial) return;
    const change = check();
    if (!change) return;
    const yes = h('button', { class: 'btn danger', type: 'button' }, 'Apply without a way back');
    const no = h('button', { class: 'btn', type: 'button' }, 'Cancel');
    yes.addEventListener('click', async () => {
      confirm.hidden = true;
      await guarded(() => run(change, null, applyButton));
    });
    no.addEventListener('click', () => {
      confirm.hidden = true;
      applyButton.focus();
    });
    confirm.replaceChildren(icon('warn'), h('div', {},
      h('p', {}, 'Apply without a way back? The rules are not undone by themselves. If this cuts off the console, change them at the machine\'s screen or over the serial console.'),
      h('div', { class: 'btns' }, yes, no)));
    confirm.hidden = false;
    no.focus();
  });

  cancelButton.addEventListener('click', () => close(true));
  element.addEventListener('keydown', (event) => {
    if (event.key === 'Escape') {
      event.preventDefault();
      close(true);
    }
  });

  // ---- opening and closing ---------------------------------------------------------

  /** Closes the form; `focus` gives focus back to what opened it. */
  function close(focus) {
    clearTimeout(commandTimer);
    element.hidden = true;
    mode = null;
    history.replaceState(history.state, '', closedAddress());
    if (focus) focusBack(opener);
    opener = null;
  }

  /** Draws the choices of On from the cards the machine has, keeping the one chosen. */
  function drawCards() {
    const names = [...new Set([...context.cards, ...(form.on !== '' && !context.cards.includes(form.on) ? [form.on] : [])])];
    on.replaceChildren(h('option', { value: '' }, 'Any card'), names.map((name) => h('option', { value: name }, name)));
    on.value = form.on;
  }

  /** The form's text for the mode: its heading and what it says before the fields. */
  function words() {
    if (mode.kind === 'add') return { title: 'Add a rule', lead: 'A new rule goes at the end, so the rules above it decide first.' };
    const old = oldRule();
    const title = mode.kind === 'replace' ? `Replace rule ${mode.number}` : `Remove rule ${mode.number}`;
    return { title, lead: old === null ? 'That rule is not there any more.' : `${wordsOf(old)}.` };
  }

  return {
    element,
    close: () => close(false),
    isOpen: () => mode !== null,
    /** Opens the form for `next`; `trigger` is the button that opened it, which gets focus back. */
    open(next, trigger = document.activeElement) {
      opener = trigger;
      mode = next;
      form = blankForm();
      const old = mode.kind === 'add' ? null : context.rules[mode.number - 1];
      if (mode.kind === 'replace' && old) form = formOfRule(old, context.here);
      if (mode.kind === 'add' && mode.words) {
        const found = parseRule(mode.words.split(' '));
        if (found.rule) form = formOfRule(found.rule, context.here);
      }
      seconds = DEFAULT_SECONDS;
      windowSelect.value = String(seconds);
      const text = words();
      heading.textContent = text.title;
      lead.textContent = text.lead;
      fields.hidden = mode.kind === 'remove';
      drawCards();
      showForm();
      clearAll();
      confirm.hidden = true;
      element.hidden = false;
      history.replaceState(history.state, '', `${closedAddress()}#add${mode.kind === 'add' ? '' : `?${mode.kind}=${mode.number}`}`);
      drawState();
      heading.focus();
    },
    /** Takes the page's newest reading of the rules, the cards and the trial. */
    set(next) {
      context = next;
      if (mode === null) return;
      // A trial that began closes the form: the rules are off until it ends.
      if (context.trial) {
        close(false);
        return;
      }
      drawCards();
      mark();
      drawState();
    },
  };
}
