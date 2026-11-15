// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The editor of one card's settings (design 9.4): DHCP or fixed addresses, the
// router, the name servers and metered, tried for a window that is undone by
// itself unless it is kept, or applied without that. The checks are the
// command's (network-model.js); a field that fails has its sentence under it.
// When the change takes away the address the browser comes by (design 8.5),
// the page says so before it is made, and Apply without trying is not offered.
// A reader sees the settings and no form.

import {
  afterPress, clearError, errorSlot, fill, h, icon, uniqueId,
} from '../../dom.js';
import { windowsFor } from '../../lib/trial-model.js';
import { factsOf } from './cards.js';
import {
  ADDRESSES_MAX, argsOf, cardRow, checkSet, cutsOff, cutWords, formOf, METERED, newAddressUrl, prefixHint, setOf,
} from './network-model.js';

/** How long after the last key the command shown under the form is made again. */
const COMMAND_MS = 300;

/** What a card that serves DHCP says of the choice it does not have (NET-178). */
const SERVES_WORDS = 'This card serves DHCP to other machines, so it needs an address of its own.';

/** The words of the confirmation Apply without trying asks (design 9.4). */
const APPLY_WORDS = 'Apply without a way back? If this cuts off the console, change it at the machine\'s screen or over the serial console.';

/** A radio card: a labelled radio button in the box the console draws for them. */
function radioCard(name, value, title, note) {
  const input = h('input', { type: 'radio', name, value });
  const label = h('label', { class: 'radio-card' }, input, h('span', {}, h('b', {}, title), note && h('small', { class: 'hint' }, ` ${note}`)));
  return { input, label };
}

/** Puts `why` under `input`, in the slot made by `errorSlot`, and marks the input. */
function showWhy(slot, input, why) {
  slot.replaceChildren(icon('warn'), h('span', {}, why));
  input.classList.add('bad');
  input.setAttribute('aria-invalid', 'true');
  input.setAttribute('aria-describedby', [input.dataset.hint, slot.id].filter(Boolean).join(' '));
}

/** Takes what `showWhy` put away, keeping the hint the input is described by. */
function clearWhy(slot, input) {
  clearError(slot, input);
  if (input.dataset.hint) input.setAttribute('aria-describedby', input.dataset.hint);
}

/**
 * The editor for the card `name`. `banner` is the trial banner of the page;
 * `reread()` reads `network show` again. Gives `{element, set(card, trial)}`,
 * where `card` is the card's entry of `network show` and `trial` says whether
 * a trial runs, which turns the form off.
 */
export function makeEditor(view, name, { banner, reread }) {
  let card = null;
  let form = null;
  let baseKey = null;
  let dirty = false;
  let trial = false;
  let seconds = 60;
  let commandTimer = null;

  const now = h('section', { class: 'card net-now', 'aria-labelledby': 'net-now-h' });
  const element = h('div', { class: 'net-editor' }, now);
  const reader = view.session.admin !== true;

  // ---- the form, made once the card is known ---------------------------------

  const dhcp = radioCard('net-mode', 'dhcp', 'From the router (DHCP)', 'The card asks for its address when it starts.');
  const fixed = radioCard('net-mode', 'fixed', 'Fixed', 'You choose the addresses.');
  const servesNote = h('p', { class: 'hint', id: 'net-serves', hidden: true }, SERVES_WORDS);
  const mode = h('fieldset', { class: 'choices net-mode' },
    h('legend', {}, 'Gets its address'), h('div', { class: 'cards-row' }, dhcp.label, fixed.label), servesNote);

  const addressBox = h('div', { class: 'net-addresses' });
  const addMore = h('button', { class: 'btn small', type: 'button' }, 'Add another');
  const router = h('input', { id: 'net-router', class: 'input mono', type: 'text', autocomplete: 'off', spellcheck: 'false' });
  const routerError = errorSlot('net-router-error');
  const dns = h('input', { id: 'net-dns', class: 'input mono', type: 'text', autocomplete: 'off', spellcheck: 'false' });
  const dnsError = errorSlot('net-dns-error');
  const fixedBox = h('div', { class: 'net-fixed' },
    h('div', { class: 'field' }, h('span', { class: 'net-label' }, 'Addresses'), addressBox, addMore),
    h('div', { class: 'net-two' },
      h('div', { class: 'field' }, h('label', { for: router.id }, 'Router (optional)'), router, routerError),
      h('div', { class: 'field' },
        h('label', { for: dns.id }, 'Name servers (up to three)'), dns,
        h('span', { class: 'hint', id: 'net-dns-hint' }, 'Separate them with commas or spaces.'), dnsError)));
  dns.dataset.hint = 'net-dns-hint';

  const metered = METERED.map((choice) => ({ choice, ...radioCard('net-metered', choice.id, choice.label) }));
  const meteredBox = h('fieldset', { class: 'choices net-metered' },
    h('legend', {}, 'Metered'),
    h('div', { class: 'cards-row' }, metered.map((one) => one.label)),
    h('p', { class: 'hint' }, 'A metered card waits to download updates and other large things until a time you choose.'));

  const windowSelect = h('select', { id: 'net-window', class: 'input net-window' });
  const windowBox = h('div', { class: 'field' },
    h('label', { for: windowSelect.id }, 'Undo by itself unless kept within'), windowSelect,
    h('span', { class: 'hint' }, 'Choose a longer time when you need it to reconnect a VPN or a phone.'));

  const cutBox = h('div', { class: 'net-cut' });
  const runs = h('div', { class: 'net-runs', hidden: true });
  const problems = h('div', { class: 'net-problems' });
  const confirm = h('div', { class: 'banner warn net-confirm', role: 'alert', hidden: true });
  const tryButton = h('button', { class: 'btn default', type: 'submit' }, 'Try it');
  const applyButton = h('button', { class: 'btn', type: 'button' }, 'Apply without trying');
  const buttons = h('div', { class: 'btns' }, tryButton, applyButton);
  const notice = h('p', { class: 'note', role: 'status' });
  const sheet = h('fieldset', { class: 'net-sheet' }, h('legend', { class: 'sr-only' }, 'Settings'), mode, fixedBox, meteredBox, windowBox);
  const formElement = h('form', { class: 'card net-form', 'aria-labelledby': 'net-form-h', novalidate: true },
    h('h2', { id: 'net-form-h' }, 'Change the settings'),
    notice, sheet, cutBox, runs, confirm, problems, buttons);

  /** The address fields, one for each address of the form; the last is never taken away when it is the only one. */
  let addressFields = [];

  function drawAddresses() {
    addressFields = form.addresses.map((text, at) => {
      const id = uniqueId('net-address');
      const hint = h('span', { id: `${id}-hint`, class: 'hint' }, prefixHint(text));
      const input = h('input', {
        id, class: 'input mono', type: 'text', value: text, autocomplete: 'off', spellcheck: 'false', 'data-hint': hint.id, 'aria-describedby': hint.id,
      });
      const error = errorSlot(`${id}-error`);
      input.addEventListener('input', () => {
        form.addresses[at] = input.value;
        hint.textContent = prefixHint(input.value);
        changed(input, error);
      });
      const remove = h('button', { class: 'btn small', type: 'button', 'aria-label': `Take away address ${at + 1}` }, 'Take away');
      remove.addEventListener('click', () => {
        form.addresses.splice(at, 1);
        dirty = true;
        drawAddresses();
        changed();
        (addressFields[Math.min(at, addressFields.length - 1)]?.input ?? addMore).focus();
      });
      const wrap = h('div', { class: 'field net-address' },
        h('label', { for: id }, `Address ${at + 1}`),
        h('div', { class: 'net-address-row' }, input, form.addresses.length > 1 && remove),
        hint, error);
      return { wrap, input, error };
    });
    fill(addressBox, addressFields.map((one) => one.wrap));
    addMore.hidden = form.addresses.length >= ADDRESSES_MAX;
  }

  addMore.addEventListener('click', () => {
    form.addresses.push('');
    dirty = true;
    drawAddresses();
    changed();
    addressFields.at(-1).input.focus();
  });

  /** Puts the values of the form's model into the fields. */
  function fillFields() {
    dhcp.input.checked = form.mode === 'dhcp';
    fixed.input.checked = form.mode === 'fixed';
    for (const one of metered) one.input.checked = form.metered === one.choice.id;
    router.value = form.router;
    dns.value = form.dns.join(', ');
    drawAddresses();
    markChoices();
  }

  /** Shows which radio cards are chosen, and the fixed fields only when Fixed is. */
  function markChoices() {
    for (const one of [dhcp, fixed, ...metered]) one.label.classList.toggle('on', one.input.checked);
    fixedBox.hidden = form.mode !== 'fixed';
  }

  // ---- what the form says -------------------------------------------------------

  /** The wrong field's place for `bad`, as `{input, slot}`; none when it belongs under the form. */
  function placeOf(bad) {
    if (bad.field === 'router') return { input: router, slot: routerError };
    if (bad.field === 'dns') return { input: dns, slot: dnsError };
    if (bad.field !== 'addresses') return null;
    // Blank fields are left out of the check, so the place of an address among those that are not is found by counting.
    const filled = addressFields.filter((one) => one.input.value.trim() !== '');
    const found = bad.at === undefined ? addressFields[0] : filled[bad.at];
    return found ? { input: found.input, slot: found.error } : null;
  }

  /** Takes every sentence away. */
  function clearAll() {
    clearWhy(routerError, router);
    clearWhy(dnsError, dns);
    for (const one of addressFields) clearWhy(one.error, one.input);
    problems.replaceChildren();
  }

  /** Checks the form as `network set` would; says what is wrong under its field and moves to it. Gives whether it is right. */
  function check() {
    clearAll();
    const bad = checkSet(setOf(form, card));
    if (bad === null) return true;
    const place = placeOf(bad);
    if (place) {
      showWhy(place.slot, place.input, bad.why);
      place.input.focus();
    } else {
      problems.replaceChildren(h('p', { class: 'msg', role: 'alert' }, icon('warn'), h('span', {}, bad.why)));
    }
    return false;
  }

  /** Says what is wrong with `input`, when it is `input` that is wrong, as the person leaves it; focus stays where they put it. */
  function checkLeft(input) {
    const bad = checkSet(setOf(form, card));
    const place = bad === null ? null : placeOf(bad);
    if (place?.input === input) showWhy(place.slot, input, bad.why);
  }

  /** The way the change takes away what the browser comes by, or null. */
  const cut = () => cutsOff(form, card, view.session);

  /** Draws what depends on the values: the warning of a cut, the windows, Apply without trying and the command. */
  function drawState() {
    const away = cut();
    const { choices, chosen } = windowsFor(away !== null, location.hostname);
    const wanted = choices.some((one) => one.seconds === seconds) ? seconds : chosen;
    const options = choices.map((one) => `${one.seconds}:${one.label}`).join('|');
    if (windowSelect.dataset.options !== options) {
      windowSelect.dataset.options = options;
      fill(windowSelect, choices.map((one) => h('option', { value: String(one.seconds) }, one.label)));
    }
    seconds = wanted;
    windowSelect.value = String(seconds);
    if (away === null) {
      cutBox.replaceChildren();
      delete cutBox.dataset.words;
    } else {
      const paragraphs = cutWords(away, { host: location.hostname, url: newAddressUrl(form, card, view.session, location) });
      // The warning is an alert when it first appears, and is not read again each time the values change.
      const same = cutBox.dataset.words === paragraphs.join('\n');
      if (!same) {
        cutBox.dataset.words = paragraphs.join('\n');
        cutBox.replaceChildren(h('div', { class: 'banner warn', role: 'alert' }, icon('warn'), h('div', {}, paragraphs.map((words) => h('p', {}, words)))));
      }
    }
    applyButton.hidden = away !== null;
    if (away !== null) confirm.hidden = true;
    drawCommand();
  }

  /** The command, made 300 ms after the last change, while Show commands is on. */
  function drawCommand() {
    runs.hidden = view.prefs?.commands !== 'shown';
    clearTimeout(commandTimer);
    if (runs.hidden) return;
    commandTimer = setTimeout(() => {
      const bad = checkSet(setOf(form, card));
      runs.replaceChildren(h('h3', {}, 'This runs'),
        bad === null ? view.api.panel('network.set', argsOf(form, card, seconds)) : h('p', { class: 'muted' }, 'The command shows when every field is right.'));
    }, COMMAND_MS);
  }
  view.onPrefs(drawCommand);
  view.signal.addEventListener('abort', () => clearTimeout(commandTimer), { once: true });

  /** A field changed: its sentence goes, and what depends on the values is drawn again. */
  function changed(input, slot) {
    dirty = true;
    if (input) clearWhy(slot, input);
    confirm.hidden = true;
    drawState();
  }

  dhcp.input.addEventListener('change', () => {
    form.mode = 'dhcp';
    markChoices();
    changed();
  });
  fixed.input.addEventListener('change', () => {
    form.mode = 'fixed';
    markChoices();
    changed();
  });
  for (const one of metered) {
    one.input.addEventListener('change', () => {
      form.metered = one.choice.id;
      markChoices();
      changed();
    });
  }
  router.addEventListener('input', () => {
    form.router = router.value;
    changed(router, routerError);
  });
  dns.addEventListener('input', () => {
    form.dns = [dns.value];
    changed(dns, dnsError);
  });
  windowSelect.addEventListener('change', () => {
    seconds = Number(windowSelect.value);
    drawCommand();
  });
  // A wrong value is said when the person leaves the field, not at each key,
  // and after the press that left it, which the sentence would otherwise move
  // the buttons under.
  formElement.addEventListener('focusout', (event) => {
    const left = event.target;
    if (left.matches?.('input[type="text"]')) afterPress(() => checkLeft(left));
  });

  // ---- the two ways to make the change ------------------------------------------

  /** The names the server's errors are put under: a field of the action. */
  const fieldsOf = () => ({ addresses: addressFields[0]?.input, router, dns });

  /** Runs `network.set` for the form, with `tried` seconds of a trial or none; gives whether it was made. */
  async function run(tried, button) {
    if (!check()) return false;
    const away = cut();
    const url = away === null ? null : newAddressUrl(form, card, view.session, location);
    // A try that takes this browser's address away has its answer given up
    // after a while, since there is no way back for it, and the banner shows
    // the trial as asked for. An Apply without a way back is waited for.
    const cutsOff = away !== null && tried !== null;
    // What Spark undid before, so that the end of a trial whose answer is lost is told from theirs.
    if (cutsOff) await banner.read();
    const result = await view.api.run('network.set', argsOf(form, card, tried), {
      button,
      words: {
        unlock: `change the network settings of ${card.name}`,
        running: tried === null ? 'Applying the settings' : 'Trying the settings',
        done: tried === null ? 'The settings are applied.' : 'The settings are applied for a trial.',
        busy: tried === null ? 'Applying…' : 'Trying…',
      },
      where: { slot: problems, fields: fieldsOf() },
      cutsOff,
    });
    if (cutsOff && result.lost) {
      dirty = false;
      banner.lost({ seconds: tried, sentAt: result.sentAt, card: card.name, url });
      return true;
    }
    if (!result.ok) return false;
    dirty = false;
    if (tried !== null) banner.begin({ seconds: tried, card: card.name, url });
    reread();
    return true;
  }

  /** Holds both buttons while `work` runs, and leaves them off when a trial now runs. */
  async function guarded(work) {
    tryButton.disabled = true;
    applyButton.disabled = true;
    try {
      return await work();
    } finally {
      tryButton.disabled = trial;
      applyButton.disabled = trial;
    }
  }

  formElement.addEventListener('submit', async (event) => {
    event.preventDefault();
    if (trial) return;
    await guarded(() => run(seconds, tryButton));
  });

  applyButton.addEventListener('click', () => {
    if (trial || !check()) return;
    const yes = h('button', { class: 'btn danger', type: 'button' }, 'Apply without a way back');
    const no = h('button', { class: 'btn', type: 'button' }, 'Cancel');
    yes.addEventListener('click', async () => {
      confirm.hidden = true;
      await guarded(() => run(null, applyButton));
    });
    no.addEventListener('click', () => {
      confirm.hidden = true;
      applyButton.focus();
    });
    confirm.replaceChildren(icon('warn'), h('div', {}, h('p', {}, APPLY_WORDS), h('div', { class: 'btns' }, yes, no)));
    confirm.hidden = false;
    no.focus();
  });

  // ---- drawn each time the card is read -----------------------------------------

  /** Turns the form off while a trial runs, and says why. */
  function drawTrial() {
    sheet.disabled = trial;
    tryButton.disabled = trial;
    applyButton.disabled = trial;
    notice.textContent = trial ? 'Keep or undo the change being tried first.' : '';
    // A card that serves DHCP cannot become a DHCP client (NET-178).
    const serves = card.settings?.dhcp_server === true;
    dhcp.input.disabled = serves || trial;
    servesNote.hidden = !serves;
    dhcp.input.setAttribute('aria-describedby', serves ? servesNote.id : '');
    if (!serves) dhcp.input.removeAttribute('aria-describedby');
  }

  return {
    element,
    set(entry, running) {
      trial = running;
      card = entry;
      const row = cardRow(entry, view.session, Date.now(), view.time);
      now.replaceChildren(h('h2', { id: 'net-now-h' }, 'Now', h('span', { class: `st ${row.state.kind}` }, row.state.word)),
        row.through && h('p', { class: 'note' }, 'You are connected through this card.'), factsOf(row));
      if (reader) {
        element.replaceChildren(now, h('p', { class: 'note' }, 'Administrators change the network settings.'));
        return;
      }
      if (!element.contains(formElement)) element.append(formElement);
      const base = formOf(entry);
      const key = JSON.stringify(base);
      // What the person typed stays while they are at it; the card's own values come back when they are not.
      if (form === null || (!dirty && key !== baseKey)) {
        form = base;
        baseKey = key;
        fillFields();
        drawState();
      } else if (key !== baseKey) {
        baseKey = key;
      }
      drawTrial();
    },
  };
}
