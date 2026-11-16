// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Devices card says, as plain values: the devices of `hardware
// devices` in groups, each with what it is to the system, its driver, its
// state and where it is, and those that work and need nothing apart. The
// system names a driver only where it can tell, so a device it cannot tell
// about is "Not told", and "No driver" is said only where it knows none took
// the device.

/** The groups, in the order the page lists them: the word the answer uses and the title. */
const GROUPS = Object.freeze([
  ['network', 'Network'], ['storage', 'Storage'], ['usb', 'USB'], ['display', 'Display'], ['other', 'Other'],
]);

/** How a state of the answer is said, and its tone. */
const STATES = Object.freeze({
  working: { word: 'Working', tone: 'ok' },
  'no-link': { word: 'No cable', tone: 'warn' },
});

/** The driver and the state of a device, as `{driver: {text, muted}, state: {word, tone}}`. */
function conditionOf(device) {
  const told = device.driver_told !== false;
  const driver = typeof device.driver === 'string' && device.driver !== '' ? device.driver : null;
  const known = STATES[device.state] ?? (typeof device.state === 'string' ? { word: device.state, tone: 'off' } : null);
  if (driver === null) {
    return {
      driver: { text: told ? 'none' : 'Not told', muted: true },
      state: known ?? (told ? { word: 'No driver', tone: 'off' } : null),
    };
  }
  return { driver: { text: driver }, state: known ?? STATES.working };
}

/** A device as a row: `{name, small, driver, state, where}`. */
function rowOf(device) {
  const names = Array.isArray(device.names) ? device.names.filter(Boolean).join(', ') : '';
  return {
    name: device.name || device.ids || 'Unnamed device',
    small: [names, device.ids].filter(Boolean).join(' · '),
    ...conditionOf(device),
    where: device.where ?? '',
  };
}

/**
 * The devices of `answer` (`hardware devices`): `{groups: [{title, rows}],
 * hidden: [row]}`. A group with no device is left out; the devices that
 * the answer marks hidden (bridges, hubs) are in `hidden` and not in a group.
 */
export function devicesOf(answer) {
  const devices = Array.isArray(answer?.devices) ? answer.devices : [];
  const found = devices.map((device) => ({
    row: rowOf(device), hidden: device.hidden === true, group: GROUPS.some(([id]) => id === device.group) ? device.group : 'other',
  }));
  return {
    groups: GROUPS.map(([id, title]) => ({ title, rows: found.filter((one) => !one.hidden && one.group === id).map((one) => one.row) })).filter((group) => group.rows.length > 0),
    hidden: found.filter((one) => one.hidden).map((one) => one.row),
  };
}

/** The summary of the devices that are folded away. */
export const hiddenWords = (count) => (count === 1
  ? 'Hidden: 1 device that works and needs nothing, such as a USB hub or a bridge'
  : `Hidden: ${count} devices that work and need nothing, such as USB hubs and bridges`);
export const NO_DEVICES = 'The system lists no device.';
