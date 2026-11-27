// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Battery and UPS card says, as plain values, from `hardware power`
// (ADR 0741): each UPS and battery with its state as a pill, its charge, how
// long the charge lasts, and for a UPS the load and when the machine shuts
// down. AC adapters and USB-C supplies are not listed: whether the mains is
// there is said on the UPS or battery it feeds. A machine with neither has
// no card.

/** `90 s`, `25 min`, `1 h 05 min`, as `hardware power` says them. */
export function durationWords(seconds) {
  if (!Number.isFinite(seconds) || seconds < 0) return null;
  if (seconds < 120) return `${seconds} s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)} min`;
  return `${Math.floor(seconds / 3600)} h ${String(Math.floor(seconds / 60) % 60).padStart(2, '0')} min`;
}

const number = (value) => (Number.isFinite(value) ? value : null);

/** The pill of a supply: `{word, tone}`. */
function pillOf(supply) {
  const percent = number(supply.percent);
  const onBattery = supply.kind === 'ups' ? supply.online === false : supply.status === 'Discharging';
  const low = onBattery && percent !== null && number(supply.low_percent) !== null && percent <= supply.low_percent;
  if (low) return { word: 'Battery low', tone: 'bad' };
  if (supply.replace === true) return { word: 'Replace battery', tone: 'bad' };
  if (onBattery) return { word: 'On battery', tone: 'warn' };
  if (supply.kind === 'ups' && supply.online === true) return { word: 'On mains', tone: 'ok' };
  if (supply.status === 'Full' || supply.status === 'Charging') return { word: supply.status, tone: 'ok' };
  return null;
}

/** The name of a supply: its model and maker, else what it is. */
function titleOf(supply) {
  const what = supply.kind === 'ups' ? 'UPS' : 'Battery';
  const model = [supply.model, supply.maker && `by ${supply.maker}`].filter(Boolean).join(' ');
  return model ? `${what} · ${model}` : what;
}

/** What a UPS's battery is doing, in words. */
function stateWords(supply) {
  if (supply.kind === 'ups') {
    if (supply.online === false) return 'The mains is gone: the UPS runs this machine on its battery.';
    if (supply.online === true) {
      return supply.status === 'Charging' ? 'On the mains, charging its battery.' : 'On the mains.';
    }
    return null;
  }
  const words = {
    Charging: 'Charging.', Discharging: 'Running this machine.', Full: 'Full, on the mains.', 'Not charging': 'On the mains, not charging.',
  };
  return words[supply.status] ?? null;
}

/** When the machine shuts down for a UPS, in words. */
function shutdownWords(supply) {
  const at = [
    number(supply.low_percent) !== null && `${supply.low_percent}%`,
    number(supply.low_seconds) !== null && `${durationWords(supply.low_seconds)} left`,
  ].filter(Boolean).join(' or ');
  if (supply.shuts_down === false) {
    return `Its battery is low at ${at || 'what the UPS says'}, but this machine stays on: it was started with ups=show.`;
  }
  return `On its battery, this machine shuts down cleanly at ${at || 'the low battery the UPS says'}.`;
}

/**
 * The rows of a supply: `{name, title, pill, rows: [[label, words]], note,
 * replace}`. A value the supply does not say is left out.
 */
function supplyOf(supply) {
  const percent = number(supply.percent);
  const seconds = number(supply.seconds_left);
  const load = number(supply.load);
  const rows = [
    ['State', stateWords(supply)],
    ['Charge', percent !== null && `${percent}%`],
    ['Lasts', seconds !== null && `About ${durationWords(seconds)} at the load now`],
    ['Load', load !== null && `${load}% of what the UPS gives`],
  ].filter(([, words]) => words);
  return {
    name: supply.name,
    title: titleOf(supply),
    pill: pillOf(supply),
    percent,
    rows,
    note: supply.kind === 'ups' ? shutdownWords(supply) : null,
    replace: supply.replace === true
      ? (supply.kind === 'ups' ? 'The UPS says its battery should be replaced (HW-142).' : 'The battery should be replaced.')
      : null,
  };
}

/** The UPSes and batteries of `answer` (`hardware power`), the UPSes first, as `supplyOf` gives them; [] for none. */
export function suppliesOf(answer) {
  const supplies = Array.isArray(answer?.supplies) ? answer.supplies : [];
  const kept = supplies.filter((one) => one?.kind === 'ups' || one?.kind === 'battery');
  return [...kept.filter((one) => one.kind === 'ups'), ...kept.filter((one) => one.kind !== 'ups')].map(supplyOf);
}

/** The card's pill: the worst of its supplies'. */
export function cardPill(supplies) {
  const order = ['bad', 'warn', 'ok'];
  const pills = supplies.map((one) => one.pill).filter(Boolean);
  return order.map((tone) => pills.find((pill) => pill.tone === tone)).find(Boolean) ?? null;
}
