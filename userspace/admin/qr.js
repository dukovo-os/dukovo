// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A QR code encoder for the authenticator app's otpauth:// address and a
// WireGuard device's settings, as ISO/IEC 18004 describes it: byte mode,
// error correction level M and versions 1 to 15, which hold up to 412
// bytes. The console loads nothing from outside its own files, so no
// library is taken from anywhere.

// For each version, the error correction codewords of every block and the
// blocks as [count, data codewords] pairs. The later blocks of a version
// hold one data codeword more than the earlier ones.
const BLOCKS = [
  null,
  { ecc: 10, groups: [[1, 16]] },
  { ecc: 16, groups: [[1, 28]] },
  { ecc: 26, groups: [[1, 44]] },
  { ecc: 18, groups: [[2, 32]] },
  { ecc: 24, groups: [[2, 43]] },
  { ecc: 16, groups: [[4, 27]] },
  { ecc: 18, groups: [[4, 31]] },
  { ecc: 22, groups: [[2, 38], [2, 39]] },
  { ecc: 22, groups: [[3, 36], [2, 37]] },
  { ecc: 26, groups: [[4, 43], [1, 44]] },
  { ecc: 30, groups: [[1, 50], [4, 51]] },
  { ecc: 22, groups: [[6, 36], [2, 37]] },
  { ecc: 22, groups: [[8, 37], [1, 38]] },
  { ecc: 24, groups: [[4, 40], [5, 41]] },
  { ecc: 24, groups: [[5, 41], [5, 42]] },
];

// The rows and columns the alignment patterns are centred on, by version.
const ALIGNMENT = [
  null, [], [6, 18], [6, 22], [6, 26], [6, 30], [6, 34],
  [6, 22, 38], [6, 24, 42], [6, 26, 46], [6, 28, 50],
  [6, 30, 54], [6, 32, 58], [6, 34, 62], [6, 26, 46, 66], [6, 26, 48, 70],
];

// The mask patterns: a module is flipped where the test holds.
const MASKS = [
  (r, c) => (r + c) % 2 === 0,
  (r) => r % 2 === 0,
  (r, c) => c % 3 === 0,
  (r, c) => (r + c) % 3 === 0,
  (r, c) => (Math.floor(r / 2) + Math.floor(c / 3)) % 2 === 0,
  (r, c) => ((r * c) % 2) + ((r * c) % 3) === 0,
  (r, c) => (((r * c) % 2) + ((r * c) % 3)) % 2 === 0,
  (r, c) => (((r + c) % 2) + ((r * c) % 3)) % 2 === 0,
];

// The field of the error correction codes: bytes as polynomials over
// GF(2), reduced by x^8 + x^4 + x^3 + x^2 + 1, with a = 2 as the generator.
const EXP = new Uint8Array(510);
const LOG = new Uint8Array(256);
for (let power = 0, value = 1; power < 255; power++) {
  EXP[power] = EXP[power + 255] = value;
  LOG[value] = power;
  value = (value << 1) ^ (value >= 128 ? 0x11d : 0);
}

function multiply(a, b) {
  return a && b ? EXP[LOG[a] + LOG[b]] : 0;
}

/**
 * The `degree` error correction codewords of `data`: the remainder of the
 * data, as a polynomial times x^degree, divided by (x - a^0) ... (x -
 * a^(degree - 1)).
 */
export function errorCorrection(data, degree) {
  let divisor = [1];
  for (let power = 0; power < degree; power++) {
    const next = new Array(divisor.length + 1).fill(0);
    divisor.forEach((coefficient, i) => {
      next[i] ^= coefficient;
      next[i + 1] ^= multiply(coefficient, EXP[power]);
    });
    divisor = next;
  }
  const rest = new Array(degree).fill(0);
  for (const byte of data) {
    const factor = byte ^ rest.shift();
    rest.push(0);
    divisor.slice(1).forEach((coefficient, i) => {
      rest[i] ^= multiply(coefficient, factor);
    });
  }
  return rest;
}

function dataCodewords(version) {
  return BLOCKS[version].groups.reduce((sum, [count, size]) => sum + count * size, 0);
}

// The smallest version that holds `length` bytes: the mode and count
// indicators take 12 bits, or 20 from version 10.
function versionFor(length) {
  for (let version = 1; version < BLOCKS.length; version++) {
    const header = version < 10 ? 12 : 20;
    if (header + 8 * length <= 8 * dataCodewords(version)) return version;
  }
  throw new RangeError(`${length} bytes do not fit a QR code of version ${BLOCKS.length - 1}`);
}

// The data codewords of `bytes` in byte mode: indicators, the bytes, the
// terminator, and the pad codewords that fill the version's capacity.
function encodeData(bytes, version) {
  const total = 8 * dataCodewords(version);
  const data = new Uint8Array(total / 8);
  let length = 0;
  const push = (value, count) => {
    for (let i = count - 1; i >= 0; i--, length++) {
      data[length >> 3] |= ((value >>> i) & 1) << (7 - (length & 7));
    }
  };
  push(0b0100, 4);
  push(bytes.length, version < 10 ? 8 : 16);
  bytes.forEach((byte) => push(byte, 8));
  length += Math.min(4, total - length);
  length = (length + 7) & ~7;
  for (let pad = 0xec; length < total; pad ^= 0xec ^ 0x11) push(pad, 8);
  return data;
}

// The codewords in the order they are placed: the blocks' data, taken a
// codeword from each block in turn, then their error correction the same way.
function interleave(data, version) {
  const { ecc, groups } = BLOCKS[version];
  const blocks = [];
  let offset = 0;
  for (const [count, size] of groups) {
    for (let i = 0; i < count; i++) {
      const part = data.subarray(offset, offset + size);
      offset += size;
      blocks.push({ data: part, ecc: errorCorrection(part, ecc) });
    }
  }
  const out = [];
  const longest = Math.max(...blocks.map((block) => block.data.length));
  for (let i = 0; i < longest; i++) {
    for (const block of blocks) if (i < block.data.length) out.push(block.data[i]);
  }
  for (let i = 0; i < ecc; i++) {
    for (const block of blocks) out.push(block.ecc[i]);
  }
  return out;
}

/** The 15 format information bits of level M with `mask`: BCH(15, 5) with the standard's XOR mask. */
export function formatBits(mask) {
  let remainder = mask;
  for (let i = 0; i < 10; i++) remainder = (remainder << 1) ^ ((remainder >>> 9) * 0x537);
  return ((mask << 10) | remainder) ^ 0x5412;
}

/** The 18 version information bits of `version` (7 and above): BCH(18, 6). */
export function versionBits(version) {
  let remainder = version;
  for (let i = 0; i < 12; i++) remainder = (remainder << 1) ^ ((remainder >>> 11) * 0x1f25);
  return (version << 12) | remainder;
}

// A square of modules, and which of them are fixed: the function patterns
// and the information bits, which data and masks leave alone.
function setFixed(grid, row, col, dark) {
  grid.modules[row * grid.size + col] = dark ? 1 : 0;
  grid.fixed[row * grid.size + col] = 1;
}

function drawFormat(grid, mask) {
  const bits = formatBits(mask);
  const { size } = grid;
  const put = (row, col, i) => setFixed(grid, row, col, (bits >>> i) & 1);
  for (let i = 0; i <= 5; i++) put(i, 8, i);
  put(7, 8, 6);
  put(8, 8, 7);
  put(8, 7, 8);
  for (let i = 9; i < 15; i++) put(8, 14 - i, i);
  for (let i = 0; i < 8; i++) put(8, size - 1 - i, i);
  for (let i = 8; i < 15; i++) put(size - 15 + i, 8, i);
  setFixed(grid, size - 8, 8, true);
}

function drawVersion(grid, version) {
  const bits = versionBits(version);
  for (let i = 0; i < 18; i++) {
    const a = grid.size - 11 + (i % 3);
    const b = Math.floor(i / 3);
    setFixed(grid, b, a, (bits >>> i) & 1);
    setFixed(grid, a, b, (bits >>> i) & 1);
  }
}

function drawFunctionPatterns(version) {
  const size = 17 + 4 * version;
  const grid = { size, modules: new Uint8Array(size * size), fixed: new Uint8Array(size * size) };
  for (let i = 0; i < size; i++) {
    setFixed(grid, 6, i, i % 2 === 0);
    setFixed(grid, i, 6, i % 2 === 0);
  }
  // A finder pattern is rings of dark, light and dark around its centre,
  // and a light ring after them that separates it from the data.
  for (const [row, col] of [[3, 3], [3, size - 4], [size - 4, 3]]) {
    for (let dr = -4; dr <= 4; dr++) {
      for (let dc = -4; dc <= 4; dc++) {
        const r = row + dr;
        const c = col + dc;
        if (r < 0 || r >= size || c < 0 || c >= size) continue;
        const ring = Math.max(Math.abs(dr), Math.abs(dc));
        setFixed(grid, r, c, ring !== 2 && ring !== 4);
      }
    }
  }
  const centres = ALIGNMENT[version];
  const last = centres.length - 1;
  centres.forEach((row, i) => {
    centres.forEach((col, j) => {
      const onFinder = (i === 0 && (j === 0 || j === last)) || (i === last && j === 0);
      if (onFinder) return;
      for (let dr = -2; dr <= 2; dr++) {
        for (let dc = -2; dc <= 2; dc++) {
          setFixed(grid, row + dr, col + dc, Math.max(Math.abs(dr), Math.abs(dc)) !== 1);
        }
      }
    });
  });
  drawFormat(grid, 0);
  if (version >= 7) drawVersion(grid, version);
  return grid;
}

// Places the codewords' bits in pairs of columns that run up and down the
// symbol from the bottom right, jumping over the vertical timing pattern.
function place(grid, codewords) {
  const { size, modules, fixed } = grid;
  let bit = 0;
  for (let right = size - 1; right >= 1; right -= 2) {
    if (right === 6) right = 5;
    const upward = ((right + 1) & 2) === 0;
    for (let step = 0; step < size; step++) {
      const row = upward ? size - 1 - step : step;
      for (const col of [right, right - 1]) {
        if (fixed[row * size + col] || bit >= codewords.length * 8) continue;
        modules[row * size + col] = (codewords[bit >> 3] >>> (7 - (bit & 7))) & 1;
        bit++;
      }
    }
  }
}

function applyMask(grid, mask) {
  const { size, modules, fixed } = grid;
  for (let row = 0; row < size; row++) {
    for (let col = 0; col < size; col++) {
      if (!fixed[row * size + col] && MASKS[mask](row, col)) modules[row * size + col] ^= 1;
    }
  }
}

// The lengths of the runs of equal modules in `line`, in order.
function runLengths(line) {
  const runs = [];
  line.forEach((module, i) => {
    if (i > 0 && module === line[i - 1]) runs[runs.length - 1]++;
    else runs.push(1);
  });
  return runs;
}

// What rule N3 finds in one line: a dark, light, dark, light, dark run of
// n, n, 3n, n, n modules with light runs around it, one at least n long
// and the other at least 4n (the quiet zone counts as light, so the
// pattern at the symbol's edge has the light it needs). Each side that
// has the long run scores 40.
function finderLikePatterns(line) {
  const runs = runLengths(line);
  const size = line.length;
  const first = line[0] ? [size, ...runs] : [size + runs[0], ...runs.slice(1)];
  const padded = line[size - 1] ? [...first, size] : [...first.slice(0, -1), first.at(-1) + size];
  let found = 0;
  for (let dark = 1; dark + 5 < padded.length; dark += 2) {
    const n = padded[dark];
    const core = [1, 3, 1, 1].every((times, i) => padded[dark + 1 + i] === n * times);
    if (!core) continue;
    const before = padded[dark - 1];
    const after = padded[dark + 5];
    if (after >= 4 * n && before >= n) found++;
    if (before >= 4 * n && after >= n) found++;
  }
  return found;
}

/**
 * The mask penalty of `rows`, the standard's four rules added up: runs of
 * five or more like modules in a line (3 and 1 for each more), 2 x 2
 * blocks of one colour (3 each), patterns like the finder patterns' 1:1:3:1:1
 * (40 each), and the share of dark modules (10 for each 5% it is away
 * from half). Rule N3 is read as Nayuki's reference encoder reads it.
 */
export function penalty(rows) {
  const size = rows.length;
  const columns = rows.map((_, col) => rows.map((row) => row[col]));
  let score = 0;
  for (const line of [...rows, ...columns]) {
    for (const run of runLengths(line)) if (run >= 5) score += run - 2;
    score += 40 * finderLikePatterns(line);
  }
  for (let row = 0; row + 1 < size; row++) {
    for (let col = 0; col + 1 < size; col++) {
      const colour = rows[row][col];
      const block = [rows[row][col + 1], rows[row + 1][col], rows[row + 1][col + 1]];
      if (block.every((module) => module === colour)) score += 3;
    }
  }
  const dark = rows.reduce((sum, row) => sum + row.reduce((count, module) => count + module, 0), 0);
  const total = size * size;
  return score + 10 * (Math.ceil((10 * Math.abs(2 * dark - total)) / total) - 1);
}

/**
 * The QR code of `text`: its `version`, the `mask` chosen, and its
 * `modules` as rows of 0 (light) and 1 (dark). The mask with the lowest
 * penalty is chosen, the first of those that tie; `chosen` makes it one
 * of the eight, which the tests use to compare with another encoder.
 * Throws a RangeError when `text` is longer than version 15 holds, 412 bytes.
 */
export function qrMatrix(text, chosen) {
  const bytes = new TextEncoder().encode(text);
  const version = versionFor(bytes.length);
  const codewords = interleave(encodeData(bytes, version), version);
  const grid = drawFunctionPatterns(version);
  place(grid, codewords);
  let best = null;
  for (const mask of chosen === undefined ? MASKS.keys() : [chosen]) {
    const trial = { ...grid, modules: grid.modules.slice() };
    applyMask(trial, mask);
    drawFormat(trial, mask);
    const rows = Array.from({ length: grid.size }, (_, row) =>
      Array.from(trial.modules.subarray(row * grid.size, (row + 1) * grid.size)));
    const score = penalty(rows);
    if (best === null || score < best.score) best = { version, mask, modules: rows, score };
  }
  return best;
}

/**
 * The path of the dark modules of `modules` as SVG path data, one run
 * of a row at a time, inside a quiet zone of `quiet` light modules.
 */
export function qrPath(modules, quiet = 4) {
  let path = '';
  modules.forEach((row, r) => {
    for (let c = 0; c < row.length;) {
      if (!row[c]) {
        c++;
        continue;
      }
      let end = c;
      while (row[end]) end++;
      path += `M${c + quiet} ${r + quiet}h${end - c}v1h-${end - c}z`;
      c = end;
    }
  });
  return path;
}

/**
 * An `<svg>` of the QR code of `text`, dark modules on a light square that
 * includes the standard's quiet zone of four modules, so that a camera
 * reads it in any theme. `label` is what a screen reader says for it.
 */
export function qrSvg(text, label) {
  const { modules } = qrMatrix(text);
  const size = modules.length + 8;
  const make = (name, attributes) => {
    const element = document.createElementNS('http://www.w3.org/2000/svg', name);
    for (const [key, value] of Object.entries(attributes)) element.setAttribute(key, value);
    return element;
  };
  const svg = make('svg', {
    class: 'qr',
    viewBox: `0 0 ${size} ${size}`,
    role: 'img',
    'aria-label': label,
    'shape-rendering': 'crispEdges',
  });
  svg.append(
    make('rect', { width: size, height: size, fill: '#fff' }),
    make('path', { d: qrPath(modules), fill: '#000' }),
  );
  return svg;
}
