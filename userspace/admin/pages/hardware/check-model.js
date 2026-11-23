// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the dialog of Check the hardware says and sends: the areas as
// choices, what a quick check does and a full one adds, and the arguments
// of `hardware.check` for what was chosen. The dialog (check-dialog.js) is
// read when the button is pressed, and so is this file.

/** The areas `hardware check` has, in the order it checks them: the word the action takes, and the label of its box. */
export const AREAS = Object.freeze([
  { id: 'memory', label: 'Memory' },
  { id: 'disks', label: 'Disks' },
  { id: 'cpu', label: 'Processors' },
  { id: 'sensors', label: 'Sensors' },
  { id: 'battery', label: 'Battery' },
  { id: 'network', label: 'Network cards' },
  { id: 'errors', label: 'Hardware errors' },
  { id: 'raid', label: 'RAID controllers' },
]);

/** What a quick check does, and what a full one takes. */
export const QUICK_WORDS = 'A quick check takes about 3 minutes: each disk\'s short self-test and a read of 256 places, part of the free memory, the processors under load for a minute, the sensors, the battery, the network cards, the hardware errors the kernel has heard of, and the volumes, disks and cache batteries of the RAID controllers. Nothing is written to any disk; the server is slower while it runs.';
export const FULL_WORDS = 'A full check takes hours: each disk\'s long self-test and a read of all of it, ten minutes of load and most of the free memory. The time it takes on this machine is shown once it starts.';

/** Said when no area is chosen: nothing is sent. */
export const NO_AREA = 'Choose at least one thing to check.';

/** The arguments of `hardware.check` for the areas `chosen` (ids) and `full`; null when none is chosen. All eight are left out, which is how the action says all. */
export function checkArgs(chosen, full) {
  const picked = AREAS.filter((area) => chosen.includes(area.id)).map((area) => area.id);
  if (picked.length === 0) return null;
  return {
    ...(picked.length === AREAS.length ? {} : { areas: picked.join(' ') }),
    ...(full ? { full: true } : {}),
  };
}
