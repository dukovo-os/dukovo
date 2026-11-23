// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What `hardware errors --json` prints, as the tests of the Hardware errors card
// need it. Not a test itself, and never installed.

const plain = (value) => JSON.parse(JSON.stringify(value));

/** The counts of a record that holds nothing. */
const quiet = () => ({
  corrected: 0,
  uncorrected: 0,
  by_severity: {
    informational: 0, corrected: 0, recoverable: 0, fatal: 0,
  },
  by_kind: {
    memory: 0, pcie: 0, processor: 0, other: 0,
  },
  recorded: 0,
  dropped: 0,
  dimms: [],
  other_dimms: { corrected: 0, uncorrected: 0 },
});

/** A machine that has heard of no error. */
export const noErrors = () => plain({
  format: 1, errors: [], counts: quiet(), skipped: 0,
});

/** One error as the kernel's record gives it. */
export const error = (more = {}) => ({
  seq: 1,
  seconds: 40.002,
  source: 'ghes3',
  severity: 'corrected',
  kind: 'memory',
  count: 1,
  dimm: 0x1100,
  part: 'DIMM 0',
  what: 'memory error: type single-bit ECC, address 0x12345000',
  ...more,
});

/**
 * A machine with a firmware error from before this start, a memory device that
 * corrected 14 errors in an hour and one that was not corrected, and a PCIe
 * error that the machine went on after.
 */
export const someErrors = () => plain({
  format: 1,
  errors: [
    error({
      seq: 1, seconds: 12.345, source: 'bert', severity: 'fatal', kind: 'pcie', dimm: null, part: 'PCIe 0000:3b:00.0', what: 'PCIe error: device 0000:3b:00.0 (8086:1533), port root port',
    }),
    error({ seq: 2, seconds: 40.002 }),
    error({
      seq: 3, seconds: 3700, count: 4, source: 'mce', dimm: null, part: 'CPU 3 bank 7 channel 1', what: 'corrected error',
    }),
    error({
      seq: 4, seconds: 7300, source: 'aer', severity: 'recoverable', kind: 'pcie', dimm: null, part: '0000:00:1c.0', what: 'bad TLP',
    }),
  ],
  counts: {
    ...quiet(),
    corrected: 6,
    uncorrected: 2,
    by_severity: {
      informational: 0, corrected: 6, recoverable: 1, fatal: 1,
    },
    by_kind: {
      memory: 5, pcie: 2, processor: 1, other: 0,
    },
    recorded: 4,
    dimms: [
      {
        handle: 0x1100, locator: 'DIMM_A1', bytes: 16 * 2 ** 30, corrected: 5, uncorrected: 0,
      },
      {
        handle: 0x1101, locator: 'DIMM_B1', bytes: 16 * 2 ** 30, corrected: 0, uncorrected: 0,
      },
      {
        handle: 0x1102, locator: null, bytes: null, corrected: 1, uncorrected: 2,
      },
    ],
    other_dimms: { corrected: 0, uncorrected: 0 },
  },
  skipped: 0,
});
