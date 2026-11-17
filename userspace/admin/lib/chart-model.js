// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Usage card's charts show (design 10.3), as plain values: the
// series of `metrics.usage` summed over their instances, broken where the
// history has a gap, the peaks and averages, the words of each chart's
// label and the rows of its table. lib/chart.js draws them.
//
// `metrics.usage` answers `[{series, points: [[unix seconds, value]]}]`;
// an instance is part of a series' name, `disk.read{vda}`, and a value that
// the machine could not read is null.

/** The ranges the card offers: the word the route takes, the button's words, how a sentence names it, how long it is and the length of one point, in seconds. */
export const RANGES = Object.freeze([
  { id: '1h', label: '1 h', words: 'the last hour', span: 3600, every: 60 },
  { id: '24h', label: '24 h', words: 'the last 24 hours', span: 86400, every: 900 },
  { id: '7d', label: '7 days', words: 'the last 7 days', span: 7 * 86400, every: 7200 },
  { id: '1y', label: '1 year', words: 'the last year', span: 365 * 86400, every: 86400 },
]);

/** The range the card starts with. */
export const DEFAULT_RANGE = '24h';

/** The range with this word, or the default one for any other word. */
export const rangeOf = (id) => RANGES.find((range) => range.id === id) ?? RANGES.find((range) => range.id === DEFAULT_RANGE);

/** A line is broken where two points are further apart than this many steps. */
export const GAP_STEPS = 3;

/** The size of the drawing, in the units of its `viewBox`; the picture is stretched to the card. */
export const VIEW = Object.freeze({ width: 300, height: 72, pad: 3 });

// ---- numbers in words ----------------------------------------------------

/** `value` with one decimal under 10 and none above, and no ".0". */
function shortNumber(value) {
  if (value >= 10) return String(Math.round(value));
  return value.toFixed(1).replace(/\.0$/, '');
}

/** A share, as "18%". */
export const formatPercent = (value) => `${shortNumber(value)}%`;

/** `value` scaled down by 1000 until it is under 1000 and written with `units`. */
function scaled(value, units) {
  let number = Math.abs(value);
  let at = 0;
  // 999.5 would be written as 1000, which is 1 of the next unit.
  while (number >= 999.5 && at < units.length - 1) {
    number /= 1000;
    at += 1;
  }
  return `${value < 0 ? '-' : ''}${shortNumber(number)} ${units[at]}`;
}

/** A size in bytes, as "7.4 GB" (the units are thousands, as the disks and metrics(1) count). */
export const formatBytes = (value) => scaled(value, ['B', 'KB', 'MB', 'GB', 'TB']);

/** Bytes a second, as "2.1 MB/s". */
export const formatRate = (value) => `${formatBytes(value)}/s`;

/** Bytes a second as bits a second, the way a network is counted: "38 Mbit/s". */
export const formatBits = (value) => scaled(value * 8, ['bit/s', 'kbit/s', 'Mbit/s', 'Gbit/s', 'Tbit/s']);

/** A count, as "1,240". */
export const formatCount = (value) => Math.round(value).toLocaleString('en-US');

/** How each kind of value is written. */
export const FORMATS = Object.freeze({
  percent: formatPercent, bytes: formatBytes, rate: formatRate, bits: formatBits, count: formatCount,
});

// ---- series ---------------------------------------------------------------

/**
 * The points of `base` in `answer`, summed over its instances, as `[{at,
 * value}]` oldest first. `base` is `cpu` or `disk.read`; `disk.read{vda}`
 * and `disk.read{vdb}` are its instances. A time at which no instance has a
 * value has no point.
 */
export function combine(answer, base) {
  const sums = new Map();
  for (const one of Array.isArray(answer) ? answer : []) {
    if (one.series !== base && !String(one.series).startsWith(`${base}{`)) continue;
    for (const [at, value] of one.points ?? []) {
      if (typeof value !== 'number' || !Number.isFinite(value)) continue;
      sums.set(at, (sums.get(at) ?? 0) + value);
    }
  }
  return [...sums.entries()].sort((a, b) => a[0] - b[0]).map(([at, value]) => ({ at, value }));
}

/** `points` as runs of points that are no more than `GAP_STEPS` steps of `every` seconds apart, so that a gap in the history is drawn as one. */
export function runs(points, every) {
  const found = [];
  for (const point of points) {
    const last = found[found.length - 1];
    if (last && point.at - last[last.length - 1].at <= GAP_STEPS * every) last.push(point);
    else found.push([point]);
  }
  return found;
}

/** The average, the peak (the earliest of equal ones) and the latest of `points`: `{count, average, peak, last}`, the last two null when there are none. */
export function summarize(points) {
  if (points.length === 0) return { count: 0, average: null, peak: null, last: null };
  let peak = points[0];
  let sum = 0;
  for (const point of points) {
    sum += point.value;
    if (point.value > peak.value) peak = point;
  }
  return { count: points.length, average: sum / points.length, peak, last: points[points.length - 1] };
}

// ---- the four charts ---------------------------------------------------------

/**
 * The charts, in the order of the card. Each has its `lines`, the series
 * each draws (the second line of a chart is drawn dashed), the `kind` of
 * its values, what its `scale` is (`percent`: 0 to 100; `total`: 0 to the
 * memory the machine has; `peak`: 0 to a little over the highest value),
 * and the words that name it in a label and a table (`name`), as a link and
 * a page's title (`short`), and in a card (`now`).
 */
export const CHARTS = Object.freeze([
  { id: 'cpu', name: 'CPU', short: 'CPU', now: 'CPU now', kind: 'percent', scale: 'percent', lines: [{ series: 'cpu', label: 'CPU' }] },
  { id: 'memory', name: 'Memory used', short: 'Memory', now: 'Memory', kind: 'bytes', scale: 'total', lines: [{ series: 'memory.used', label: 'Used' }] },
  { id: 'disk', name: 'Disk reads and writes', short: 'Disk', now: 'Disk now', kind: 'rate', scale: 'peak', lines: [{ series: 'disk.read', label: 'Read' }, { series: 'disk.write', label: 'Written' }] },
  { id: 'network', name: 'Network in and out', short: 'Network', now: 'Network now', kind: 'bits', scale: 'peak', lines: [{ series: 'net.rx', label: 'In' }, { series: 'net.tx', label: 'Out' }] },
]);

/**
 * The memory the machine has, in bytes: the kernel's count from
 * `console.about` when it is there, else what the last used and available
 * values add up to, else null.
 */
export function totalMemory(answer, about) {
  if (typeof about?.memory === 'number' && about.memory > 0) return about.memory;
  const used = combine(answer, 'memory.used').at(-1);
  const free = combine(answer, 'memory.available').at(-1);
  return used && free ? used.value + free.value : null;
}

/**
 * The model of one chart for `answer`, over `range` (see `rangeOf`) ending
 * at `now`, in Unix seconds: `{chart, range, from, to, floor, ceiling,
 * lines: [{label, series, points, runs, summary}], empty}`.
 */
export function modelOf(chart, answer, { range, now, total = null }) {
  const lines = chart.lines.map((line) => {
    const points = combine(answer, line.series);
    return { ...line, points, runs: runs(points, range.every), summary: summarize(points) };
  });
  const highest = Math.max(0, ...lines.map((line) => line.summary.peak?.value ?? 0));
  let ceiling = highest > 0 ? highest * 1.1 : 1;
  if (chart.scale === 'percent') ceiling = 100;
  if (chart.scale === 'total' && total) ceiling = Math.max(total, highest);
  return {
    chart, range, from: now - range.span, to: now, floor: 0, ceiling, lines, total,
    empty: lines.every((line) => line.points.length === 0),
  };
}

/** The x of the time `at` (Unix seconds) in the drawing's units, held inside the picture. */
export function xOf(model, at) {
  const across = (at - model.from) / (model.to - model.from);
  return Math.min(VIEW.width, Math.max(0, across * VIEW.width));
}

/** The x and y of a point in the drawing's units, held inside the picture. */
function place(model, point) {
  const { height, pad } = VIEW;
  const up = (point.value - model.floor) / (model.ceiling - model.floor);
  const y = height - pad - Math.min(1, Math.max(0, up)) * (height - 2 * pad);
  return [xOf(model, point.at), y];
}

const text = (number) => String(Math.round(number * 10) / 10);

/**
 * The shapes of one line of `model`: `[{points, area}]`, one for each run.
 * `points` is for a polyline; `area` closes the shape at the bottom, for the
 * first line's fill. A run of one point is a short flat stroke, which a
 * polyline of one point would not show.
 */
export function shapes(model, line) {
  return line.runs.map((run) => {
    let placed = run.map((point) => place(model, point));
    if (placed.length === 1) placed = [[Math.max(0, placed[0][0] - 1.5), placed[0][1]], [Math.min(VIEW.width, placed[0][0] + 1.5), placed[0][1]]];
    const points = placed.map(([x, y]) => `${text(x)},${text(y)}`).join(' ');
    const area = `${points} ${text(placed[placed.length - 1][0])},${VIEW.height} ${text(placed[0][0])},${VIEW.height}`;
    return { points, area };
  });
}

// ---- words -----------------------------------------------------------------

/** "peak 72% at 2:14 AM": `summary`'s highest, in `format` and the person's time zone (`when` is `time.when`). */
const peakWords = (summary, format, when) => `peak ${format(summary.peak.value)} at ${when(summary.peak.at)}`;

/**
 * What a screen reader says of the chart, in words: "CPU over the last 24
 * hours: average 12%, peak 72% at 2:14 AM". A chart of two lines says each
 * in turn. `time` is lib/time.js's, so the times are the person's.
 */
export function labelOf(model, time) {
  const format = FORMATS[model.chart.kind];
  const head = `${model.chart.name} over ${model.range.words}`;
  if (model.empty) return `${head}: no numbers yet`;
  const parts = model.lines.map((line) => {
    if (line.points.length === 0) return `${line.label.toLowerCase()}: no numbers`;
    const words = `average ${format(line.summary.average)}, ${peakWords(line.summary, format, time.when)}`;
    return model.lines.length > 1 ? `${line.label.toLowerCase()}: ${words}` : words;
  });
  return `${head}: ${parts.join('; ')}`;
}

/**
 * The words above a chart: `{value, note}`, "18%" and "CPU now · peak 92% at
 * 3:04 AM". The value is the newest point, or the newest of the two lines
 * added, since a disk's reads and writes together are its use.
 */
export function statOf(model, time) {
  const format = FORMATS[model.chart.kind];
  if (model.empty) return { value: '–', note: `${model.chart.now ?? model.chart.name} · no numbers yet` };
  const latest = model.lines.reduce((sum, line) => sum + (line.summary.last?.value ?? 0), 0);
  const peaks = model.lines.filter((line) => line.summary.peak);
  const top = peaks.reduce((best, line) => (line.summary.peak.value > best.summary.peak.value ? line : best));
  const who = model.lines.length > 1 ? ` ${top.label.toLowerCase()}` : '';
  const peak = `peak ${format(top.summary.peak.value)}${who} at ${time.when(top.summary.peak.at)}`;
  let note = `${model.chart.now} · ${peak}`;
  if (model.chart.scale === 'total') {
    note = model.total ? `${model.chart.now} of ${formatBytes(model.total)} · average ${format(top.summary.average)}` : `${model.chart.now} used · ${peak}`;
  }
  return { value: format(latest), note };
}

/**
 * The table under "Show the numbers": `{head, rows}`, the head's words
 * and a row for each time that any line has a point, newest first, each
 * a list of the time and the values as words (an empty cell where a line
 * has none). The first column is "Time" unless the chart names it (`column`).
 */
export function tableOf(model, time) {
  const format = FORMATS[model.chart.kind];
  const times = [...new Set(model.lines.flatMap((line) => line.points.map((point) => point.at)))].sort((a, b) => b - a);
  const byTime = model.lines.map((line) => new Map(line.points.map((point) => [point.at, point.value])));
  return {
    head: [model.chart.column ?? 'Time', ...model.lines.map((line) => line.label)],
    rows: times.map((at) => [time.when(at), ...byTime.map((values) => (values.has(at) ? format(values.get(at)) : ''))]),
  };
}
