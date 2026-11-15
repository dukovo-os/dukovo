// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Disks, partitions and volumes in words (design 3.5), as plain values, from
// the object that `storage show --json` prints: each partition's share of
// its disk, what a partition is for, a disk's health, and what a volume's
// profiles say about its copies. Storage and Move use them.
//
// A volume is `{uuid, label, mount, size, used, free, devices, disks,
// missing, behind, profiles: {data, metadata, system} | null, holds, scrub}`
// and a disk `{name, model, serial, size, removable, partitions, free_after,
// health | null}`; see section 11.1 of the design.

/** The least share of its disk that a part is drawn with, in percent, so that a small partition is still seen. */
export const MIN_SHARE = 2;

/** The share of a disk that a gap after its last partition must be to be drawn, in percent. */
export const MIN_GAP = 1;

// ---- disks to scale ----------------------------------------------------------

/**
 * The parts of each disk of `disks`, in the order they lie on it, each
 * `{id, partition, free, bytes, percent}`: a partition, or the free space
 * after the last one (`free`, `partition` null), with the percent of the
 * bar it is drawn with. A part is `MIN_SHARE` percent at least, which the
 * larger parts give up, and the percents add up to 100. A disk with no
 * partition is one free part. Gives `[{name, parts}]`.
 */
export function scaled(disks) {
  return (Array.isArray(disks) ? disks : []).map((disk) => ({ name: disk.name, parts: partsOf(disk) }));
}

/** The parts of one disk: its partitions by where they start, and the free space after them when it is a share worth drawing. */
function partsOf(disk) {
  const size = disk.size > 0 ? disk.size : 0;
  const partitions = [...(disk.partitions ?? [])].sort((a, b) => a.start - b.start);
  const parts = partitions.map((partition) => ({ id: partition.name, partition, free: false, bytes: partition.size }));
  const free = partitions.length === 0 ? size : Math.max(0, disk.free_after ?? 0);
  if (free > 0 && (partitions.length === 0 || (size > 0 && (free / size) * 100 >= MIN_GAP))) {
    parts.push({ id: 'free', partition: null, free: true, bytes: free });
  }
  return share(parts, size);
}

/** `parts` with their `percent`: each one's share of `size` or `MIN_SHARE`, the rest reduced alike so that all add up to 100. */
function share(parts, size) {
  const floor = Math.min(MIN_SHARE, parts.length === 0 ? 0 : 100 / parts.length);
  const raw = parts.map((part) => (size > 0 ? (part.bytes / size) * 100 : 0));
  const total = raw.reduce((sum, value) => sum + value, 0) || 1;
  // Parts are in proportion to each other, whatever the table leaves out around them.
  const even = raw.map((value) => (value / total) * 100);
  const small = even.filter((value) => value < floor).length;
  const big = even.filter((value) => value >= floor).reduce((sum, value) => sum + value, 0);
  const room = 100 - small * floor;
  return parts.map((part, at) => ({ ...part, percent: even[at] < floor ? floor : (even[at] / big) * room }));
}

// ---- volumes ------------------------------------------------------------------

/** The profiles of a volume, which a balance may be changing: `{known, data, metadata, system}`, the last three lists (empty for a kind the volume does not say). */
function profilesOf(volume) {
  const given = volume?.profiles;
  const list = (kind) => (Array.isArray(given?.[kind]) ? given[kind] : []);
  return { known: given != null, data: list('data'), metadata: list('metadata'), system: list('system') };
}

/** The profiles that keep a second copy of a block on another disk, which a missing disk does not lose. */
const REDUNDANT = Object.freeze(['raid1', 'raid1c3', 'raid1c4', 'raid10', 'raid5', 'raid6']);

/** The profiles that keep whole copies of every block: a mirror. */
const MIRRORS = Object.freeze(['raid1', 'raid1c3', 'raid1c4', 'raid10']);

/** Whether `kinds` (the profiles in use for one kind of block group) are all among `among`; false for none. */
const allIn = (kinds, among) => kinds.length > 0 && kinds.every((kind) => among.includes(kind));

/** Whether a volume's data and its metadata are all mirrored, on at least two disks. */
export function isMirror(volume) {
  const profiles = profilesOf(volume);
  return volume?.disks > 1 && allIn(profiles.data, MIRRORS) && allIn(profiles.metadata, MIRRORS);
}

/** Which of a volume's parts it is: `system`, `system-data` (the system volume holds the people's folders too), `data` or `other`. */
export function roleOf(volume) {
  const holds = Array.isArray(volume?.holds) ? volume.holds : [];
  const data = holds.includes('/users') || holds.includes('/shared');
  if (volume?.mount === '/') return data ? 'system-data' : 'system';
  return data ? 'data' : 'other';
}

/** The title of a volume's card: "System", "System and data", "Data", else its label, else "Volume". */
export function volumeTitle(volume) {
  const role = roleOf(volume);
  if (role === 'system') return 'System';
  if (role === 'system-data') return 'System and data';
  if (role === 'data') return 'Data';
  return volume?.label || 'Volume';
}

/** How a sentence names a volume: "the system", "the data", "the system and data", "the Photos volume". */
export function volumeName(volume) {
  if (roleOf(volume) === 'other') return `the ${volume?.label ? `${volume.label} ` : ''}volume`;
  return `the ${volumeTitle(volume).toLowerCase()}`;
}

/** The disk that holds the partition named `partition`, or null. */
export function diskOfPartition(disks, partition) {
  return (Array.isArray(disks) ? disks : []).find((disk) => (disk.partitions ?? []).some((one) => one.name === partition)) ?? null;
}

/** The names of the disks that hold `volume`, in the order of its devices, each once; a device that no disk of `disks` holds is named as it is. */
export function disksOf(volume, disks = []) {
  const names = (volume?.devices ?? []).map((device) => diskOfPartition(disks, device)?.name ?? device);
  return [...new Set(names)];
}

/** "sda", "sda and sdb", "sda, sdb and sdc". */
export function listWords(names) {
  if (names.length <= 1) return names.join('');
  return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`;
}

/** The layout of a volume's data, as "Btrfs RAID1", "Btrfs single", or "Btrfs" when the volume does not say. */
export function layoutWords(volume) {
  const { data } = profilesOf(volume);
  return data.length === 0 ? 'Btrfs' : `Btrfs ${data.map((kind) => (kind === 'single' ? kind : kind.toUpperCase())).join(' and ')}`;
}

/**
 * What a volume's profiles and disks say about its copies, as `{kind, tone,
 * word, where, detail}`: the pill's `word`, the end of the stat line
 * (`where`: "on sda and sdb, each a full copy"), and a sentence of
 * `detail`. The kinds, worst first:
 *
 * - `missing`: a disk is gone. Bad.
 * - `behind`: a disk missed changes and is stale until a scrub has copied
 *   them across; "Mirror catching up" while `scrubbing` is true. Warn.
 * - `converting`: a balance is changing a profile. Info.
 * - `healthy`: a mirror with all its disks. Ok.
 * - `partial`: only the data, or only the metadata, has a second copy. Warn.
 * - `spread`: `single` or `raid0` over several disks. Warn.
 * - `dup`: two copies on one disk. Info.
 * - `disks`: the profiles could not be read; the disks alone. Info.
 * - `none`: one disk, one copy: no `word`.
 *
 * `disks` are those of `storage show`, for their names.
 */
export function mirrorWords(volume, disks = [], scrubbing = false) {
  const names = disksOf(volume, disks);
  const count = volume?.disks > 0 ? volume.disks : names.length;
  const place = names.length > 0 ? `on ${listWords(names)}` : '';
  const say = (kind, tone, word, detail = null, where = place) => ({ kind, tone, word, where, detail });
  const profiles = profilesOf(volume);
  if (volume?.missing > 0) {
    const kept = isMirror(volume);
    const gone = `${volume.missing} of ${count} disks ${volume.missing === 1 ? 'is' : 'are'} missing.`;
    return say('missing', 'bad', kept ? 'Mirror missing a disk' : 'Missing a disk', kept ? `${gone} Every file is still there, with no second copy.` : gone);
  }
  if (volume?.behind > 0) {
    const gone = `${volume.behind} of ${count} disks missed changes`;
    return scrubbing
      ? say('behind', 'warn', 'Mirror catching up', `${gone}. The check is copying them across.`)
      : say('behind', 'warn', 'Mirror behind', `${gone}. Checking every block copies them across.`);
  }
  if (!profiles.known) {
    return count > 1 ? say('disks', 'info', `On ${count} disks`) : say('none', 'off', null, null);
  }
  if ([profiles.data, profiles.metadata, profiles.system].some((kinds) => kinds.length > 1)) {
    return say('converting', 'info', 'Changing layout', 'A balance is changing how its blocks are kept. Until it ends, some may have no second copy.');
  }
  if (count > 1) {
    const weak = ['data', 'metadata'].filter((kind) => !allIn(profiles[kind], REDUNDANT));
    if (weak.length === 0) return say('healthy', 'ok', 'Mirror healthy', null, `${place ? `${place}, ` : ''}each a full copy`);
    if (weak.length === 1) return say('partial', 'warn', 'Partly mirrored', `Its ${weak[0]} has no second copy.`);
    return say('spread', 'warn', `Spread over ${listWords(names) || `${count} disks`}`, 'With no second copy: if one disk fails, files are lost.', '');
  }
  if (profiles.data.includes('dup')) {
    return say('dup', 'info', 'Two copies on one disk', 'This protects against a damaged block, not against the disk failing.');
  }
  return say('none', 'off', null, null, place);
}

// ---- what a partition is for ------------------------------------------------

/** The filesystems as a person writes them. */
const FILESYSTEMS = Object.freeze({ btrfs: 'Btrfs', vfat: 'FAT', exfat: 'exFAT', ntfs: 'NTFS', ext4: 'ext4' });

/** The volume of `volumes` that has the partition named `partition` as a device, or null. */
export function volumeOf(partition, volumes) {
  return (Array.isArray(volumes) ? volumes : []).find((volume) => (volume.devices ?? []).includes(partition)) ?? null;
}

/** `words` with a capital first letter. */
const capital = (words) => words.charAt(0).toUpperCase() + words.slice(1);

/**
 * What a partition is for, as the end of "Used for": "Start-up files",
 * "Swap", "Half of the data mirror", "The system", "Unused". `volumes` are
 * those of `storage show`.
 */
export function usedFor(partition, volumes = []) {
  if (!partition) return 'Unused';
  if (partition.type === 'efi') return 'Start-up files (the ESP)';
  if (partition.type === 'bios-boot') return 'Start-up code for older machines';
  if (partition.type === 'swap' || partition.filesystem === 'swap') return 'Swap';
  const volume = volumeOf(partition.name, volumes);
  if (volume) {
    if (isMirror(volume)) {
      // "the Photos volume" is "the Photos mirror" here, as "the data" is "the data mirror".
      return `${volume.disks === 2 ? 'Half' : `One of ${volume.disks} disks`} of ${volumeName(volume).replace(/ volume$/, '')} mirror`;
    }
    if (volume.disks > 1) return `Part of ${volumeName(volume)}, spread over ${volume.disks} disks`;
    return capital(volumeName(volume));
  }
  if (partition.filesystem) {
    const kind = FILESYSTEMS[partition.filesystem] ?? partition.filesystem;
    return `Files in the ${kind} format${partition.label ? `, called ${partition.label}` : ''}`;
  }
  return 'Unused';
}

// ---- health ---------------------------------------------------------------------

/**
 * A disk's health, `health` as `storage show` gives it (null for a disk
 * that gives none), as `{word, tone, note}`: "Healthy", "Wearing" or
 * "Failing", the tone of its pill, and one sentence that says why for the
 * last two. A disk that passed but whose counts rose lately is wearing.
 */
export function healthWords(health) {
  if (!health) return { word: 'Not checked', tone: 'off', note: 'This disk does not report its health.' };
  const reasons = [];
  const { moved, waiting, offline, media_errors: media, used_percent: used } = health;
  if (moved > 0) reasons.push(`${moved} ${moved === 1 ? 'sector' : 'sectors'} moved to spares`);
  if (waiting > 0) reasons.push(`${waiting} ${waiting === 1 ? 'sector' : 'sectors'} waiting to be moved`);
  if (offline > 0) reasons.push(`${offline} ${offline === 1 ? 'sector' : 'sectors'} that could not be read`);
  if (media > 0) reasons.push(`${media} media ${media === 1 ? 'error' : 'errors'}`);
  if (used >= 90) reasons.push(`${Math.round(used)} percent of its rated writes used`);
  const note = reasons.length === 0 ? '' : `${capital(reasons.join(', '))}.`;
  if (health.verdict === 'failing') return { word: 'Failing', tone: 'bad', note: note || 'The disk says it is failing.' };
  if (health.verdict === 'warning' || health.rose) return { word: 'Wearing', tone: 'warn', note: note || 'The disk warns that it may fail.' };
  return { word: 'Healthy', tone: 'ok', note };
}
