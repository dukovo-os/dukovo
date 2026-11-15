// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The filters of the Logs page (design 5.1, 5.7): When, Times in, Importance,
// From and Words, and a custom range's two dates and times. A change is told
// to the page as the part of the query it changes; the page reads again, or
// shows the lines in another zone. Typing in Words waits 600 ms after the
// last key, because each read as an administrator is one audit line. On a
// phone the less used ones fold under a Filters button.

import { h, uniqueId } from '../../dom.js';
import {
  PRIORITIES, wordsProblem, ZONES,
} from '../../lib/log-model.js';
import { RANGES, wallOf, wallToUtc } from '../../lib/when-model.js';

/** How long typing in Words waits for the next key before it asks. */
export const TYPING_MS = 600;

const HOUR_MS = 3_600_000;

/** A label above a field, the field and a place for what is wrong with it. */
function field(label, control, id, extra = []) {
  const problem = h('div', { id: `${id}-problem`, class: 'msg', role: 'alert' });
  return { element: h('div', { class: 'field' }, h('label', { for: id }, label), control, ...extra, problem), problem };
}

/** A `<select>` of `[value, words]` choices. */
function menu(id, choices, value) {
  return h('select', { id, class: 'input' }, ...choices.map(([word, words]) => h('option', { value: word, selected: word === value }, words)));
}

/** Marks `input` as wrong with `words` under it, or clears it when `words` is empty. Focus stays where the person is. */
function say({ problem }, input, words) {
  problem.replaceChildren(words ?? '');
  if (words) input.setAttribute('aria-invalid', 'true');
  else input.removeAttribute('aria-invalid');
  input.classList.toggle('bad', Boolean(words));
  if (words) input.setAttribute('aria-describedby', problem.id);
  else input.removeAttribute('aria-describedby');
}

/**
 * The filters, drawn for `query`. `zoneNames()` gives `{you, server}`, the
 * names of the zones the times can be shown in (`server` null when this
 * browser does not know the server's); `now()` is the time in milliseconds;
 * `onChange(change)` is called with the part of the query that a change
 * makes, `{when, since, until}`, `{priority}`, `{service, kernel}`, `{q}`,
 * `{zone}` or `{fold}`; `signal` ends a timer that waits. Gives `element`,
 * `fields` (the controls an error of the read can be shown under, by the
 * read's own names), `set(query)`, which shows another query without
 * calling `onChange`, and `setServices(names)`.
 */
export function makeFilters({
  query, zoneNames, now = () => Date.now(), onChange, signal,
}) {
  let current = query;
  let services = [];
  let waiting = null;

  const ids = { when: uniqueId('lf-when'), zone: uniqueId('lf-zone'), priority: uniqueId('lf-priority'), from: uniqueId('lf-from'), words: uniqueId('lf-words') };

  const when = menu(ids.when, RANGES.map((range) => [range.id, range.label]), query.when);
  const priority = menu(ids.priority, PRIORITIES.map((one) => [one.id, one.label]), query.priority);
  const zone = menu(ids.zone, [], query.zone);
  const from = menu(ids.from, [], '');
  const words = h('input', {
    id: ids.words, class: 'input', type: 'search', maxlength: '200', autocomplete: 'off', autocapitalize: 'off', spellcheck: 'false', placeholder: 'for example: timeout', value: query.q,
  });
  const fold = h('input', { id: uniqueId('lf-fold'), type: 'checkbox', checked: query.fold });

  const whenField = field('When', when, ids.when);
  const priorityField = field('Importance', priority, ids.priority);
  const zoneField = field('Times in', zone, ids.zone);
  const fromField = field('From', from, ids.from);
  const wordsField = field('Words in the line', words, ids.words, [h('span', { class: 'hint' }, 'Upper-case letters make the search match case.')]);

  // The custom range: a date and a time at each end, in the zone the times are shown in.
  const range = {
    sinceDate: h('input', { id: uniqueId('lf-sd'), class: 'input', type: 'date', 'aria-label': 'Start date' }),
    sinceTime: h('input', { id: uniqueId('lf-st'), class: 'input', type: 'time', step: '1', 'aria-label': 'Start time' }),
    untilDate: h('input', { id: uniqueId('lf-ud'), class: 'input', type: 'date', 'aria-label': 'End date' }),
    untilTime: h('input', { id: uniqueId('lf-ut'), class: 'input', type: 'time', step: '1', 'aria-label': 'End time' }),
  };
  const sinceField = field('Start', h('span', { class: 'when-pair' }, range.sinceDate, range.sinceTime), range.sinceDate.id);
  const untilField = field('End', h('span', { class: 'when-pair' }, range.untilDate, range.untilTime), range.untilDate.id,
    [h('span', { class: 'hint' }, 'Leave empty to read up to now.')]);
  const custom = h('div', { class: 'filters-custom', hidden: query.when !== 'custom' }, sinceField.element, untilField.element);

  const toggle = h('button', { class: 'btn filters-toggle', type: 'button', 'aria-expanded': 'false' }, 'Filters');
  const more = h('div', { class: 'filters-more', id: uniqueId('lf-more') },
    zoneField.element,
    fromField.element,
    wordsField.element,
    h('label', { class: 'check fold-check' }, fold, h('span', {}, 'Fold repeated lines')));
  toggle.setAttribute('aria-controls', more.id);
  const element = h('div', { class: 'filters', role: 'group', 'aria-label': 'Filters' },
    h('div', { class: 'filters-main' }, whenField.element, custom, priorityField.element, toggle),
    more);
  toggle.addEventListener('click', () => {
    const open = toggle.getAttribute('aria-expanded') !== 'true';
    toggle.setAttribute('aria-expanded', String(open));
    element.classList.toggle('open', open);
  });

  // ---- what the controls hold ----

  function fillZones() {
    const { you, server } = zoneNames();
    const choices = [['you', `${ZONES[0].label} (${you})`]];
    if (server) choices.push(['server', `${ZONES[1].label} (${server})`]);
    choices.push(['utc', ZONES[2].label]);
    zone.replaceChildren(...choices.map(([word, label]) => h('option', { value: word, selected: word === current.zone }, label)));
  }

  function fillFrom() {
    const named = current.service !== '' && !services.includes(current.service) ? [...services, current.service].sort() : services;
    const choices = [['all', 'Everything'], ['kernel', 'The machine itself'], ...named.map((name) => [`service:${name}`, name])];
    from.replaceChildren(...choices.map(([word, label]) => h('option', { value: word, selected: word === fromValue(current) }, label)));
  }

  const fromValue = (shown) => {
    if (shown.kernel) return 'kernel';
    return shown.service === '' ? 'all' : `service:${shown.service}`;
  };

  /** The zone the range's fields are read in: the one the times are shown in. */
  const shownZone = () => {
    if (current.zone === 'utc') return 'UTC';
    return zoneNames()[current.zone === 'server' ? 'server' : 'you'] ?? 'UTC';
  };

  function fillRange() {
    const place = (ms, date, time) => {
      const wall = ms === null ? { date: '', time: '' } : wallOf(ms, shownZone());
      date.value = wall.date;
      time.value = wall.time;
    };
    place(current.since, range.sinceDate, range.sinceTime);
    place(current.until, range.untilDate, range.untilTime);
  }

  function show(next) {
    current = next;
    when.value = next.when;
    priority.value = next.priority;
    fillZones();
    fillFrom();
    if (words.value.trim() !== next.q) words.value = next.q;
    fold.checked = next.fold;
    custom.hidden = next.when !== 'custom';
    if (next.when === 'custom') fillRange();
  }

  // ---- what a change tells ----

  /** Reads the range's fields; tells the page when both ends are fine. */
  function changedRange() {
    const named = shownZone();
    say(sinceField, range.sinceDate, null);
    say(untilField, range.untilDate, null);
    const sinceDate = range.sinceDate.value;
    const sinceTime = range.sinceTime.value || '00:00';
    if (sinceDate === '') {
      say(sinceField, range.sinceDate, 'Choose a start date.');
      return;
    }
    const since = wallToUtc(sinceDate, sinceTime, named);
    if (since === null) {
      say(sinceField, range.sinceDate, 'That start is not a date and time.');
      return;
    }
    let until = null;
    if (range.untilDate.value !== '') {
      until = wallToUtc(range.untilDate.value, range.untilTime.value || '00:00', named);
      if (until === null) {
        say(untilField, range.untilDate, 'That end is not a date and time.');
        return;
      }
      if (until <= since) {
        say(untilField, range.untilDate, 'The end must be after the start.');
        return;
      }
    }
    onChange({ when: 'custom', since, until });
  }
  for (const input of Object.values(range)) input.addEventListener('change', changedRange);

  when.addEventListener('change', () => {
    if (when.value !== 'custom') {
      custom.hidden = true;
      onChange({ when: when.value, since: null, until: null });
      return;
    }
    // A custom range starts from the last hour, and the person moves it.
    const end = Math.floor(now() / 60_000) * 60_000;
    current = { ...current, when: 'custom', since: end - HOUR_MS, until: null };
    custom.hidden = false;
    fillRange();
    onChange({ when: 'custom', since: end - HOUR_MS, until: null });
  });
  priority.addEventListener('change', () => onChange({ priority: priority.value }));
  zone.addEventListener('change', () => {
    current = { ...current, zone: zone.value };
    if (current.when === 'custom') fillRange();
    onChange({ zone: zone.value });
  });
  from.addEventListener('change', () => {
    const [kind, ...rest] = from.value.split(':');
    onChange({ service: kind === 'service' ? rest.join(':') : '', kernel: kind === 'kernel' });
  });
  fold.addEventListener('change', () => onChange({ fold: fold.checked }));

  function sendWords() {
    clearTimeout(waiting);
    waiting = null;
    const typed = words.value.trim();
    if (typed === current.q || wordsProblem(typed) !== null) return;
    current = { ...current, q: typed };
    onChange({ q: typed });
  }
  words.addEventListener('input', () => {
    clearTimeout(waiting);
    waiting = null;
    const problem = wordsProblem(words.value);
    say(wordsField, words, problem);
    if (problem === null) waiting = setTimeout(sendWords, TYPING_MS);
  });
  words.addEventListener('keydown', (event) => {
    if (event.key !== 'Enter') return;
    event.preventDefault();
    sendWords();
  });
  words.addEventListener('change', sendWords);
  words.addEventListener('search', sendWords);
  signal?.addEventListener('abort', () => clearTimeout(waiting), { once: true });

  show(query);
  return {
    element,
    fields: {
      since: range.sinceDate, until: range.untilDate, grep: words, service: from, priority,
    },
    set: show,
    setServices(names) {
      services = names;
      fillFrom();
    },
  };
}
