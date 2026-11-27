// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Input devices card says, as plain values: each keyboard, mouse,
// tablet, touch device and switch of `hardware inputs` as a row, with what
// it is, how it is connected, how often it is read and what it has. What a
// device does not say is not said here either.

const DASH = '—';

/** What a device is and how it is connected, as the table says them. */
const KINDS = { keyboard: 'Keyboard', keys: 'Keys', pointer: 'Pointer', tablet: 'Tablet', touch: 'Touch', switch: 'Switch', other: 'Other' };
const BUSES = { usb: 'USB', ps2: 'PS/2', i2c: 'I2C', virtual: 'Virtual' };
const LIGHTS = { 'num-lock': 'Num Lock', 'caps-lock': 'Caps Lock', 'scroll-lock': 'Scroll Lock' };

/** `count` of `thing`, in words: `1 key`, `3 buttons`; nothing for none. */
const many = (count, thing) => (count > 0 ? `${count} ${thing}${count === 1 ? '' : 's'}` : '');

const listOf = (value) => (Array.isArray(value) ? value : []);

/** A device's wheels, in words. */
function wheelWords(device) {
  if (device.wheel === true) return device.sideways_wheel === true ? 'a wheel that also turns sideways' : 'a wheel';
  return device.sideways_wheel === true ? 'a sideways wheel' : '';
}

/** What a device has: its keys, buttons, wheels and lights. */
function hasWords(device) {
  const lights = listOf(device.lights).map((light) => LIGHTS[light] ?? light);
  return [
    many(device.keys, 'key'), many(device.buttons, 'button'), wheelWords(device),
    lights.length > 0 && `lights for ${lights.join(', ')}`,
  ].filter(Boolean).join('; ') || DASH;
}

/** A device as a row: `{name, muted, small, is, connected, where, read, has}`. */
function rowOf(device) {
  const bus = String(device.bus ?? '');
  // A PS/2 keyboard's place says no more than its bus.
  const where = device.where && device.where !== bus ? device.where : '';
  return {
    name: device.name ?? 'A device that does not say what it is',
    muted: device.name == null,
    small: `/devices/input/${device.number}`,
    is: listOf(device.kinds).map((kind) => KINDS[kind] ?? kind).join(', ') || KINDS.other,
    connected: BUSES[bus] ?? (bus ? `Bus ${bus}` : DASH),
    where: [device.ids, where].filter(Boolean).join(' · '),
    read: typeof device.rate_hz === 'number' && device.rate_hz > 0 ? `${many(device.rate_hz, 'time')} a second` : 'When it sends',
    has: hasWords(device),
  };
}

/**
 * The input devices of `answer` (`hardware inputs`): `{held, rows}`, a row
 * for each device, and whether Ctrl+Alt+Delete has the consoles in front.
 */
export function inputsOf(answer) {
  return { held: answer?.held === true, rows: listOf(answer?.devices).map(rowOf) };
}

export const NO_INPUTS = 'No input device: the system reads no keyboard and no mouse.';
export const HELD = 'Ctrl+Alt+Delete has the consoles in front: only they read a key.';
