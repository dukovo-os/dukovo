// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Storage page says (design 4), from fixtures of `storage show
// --json`: the banner in the order of its worst, the titles and words of the
// cards, the disks to scale, a scrub's progress, and what the Overview takes.
// One disk's panel and the log's card are in storage-detail and storage-log.

import assert from 'node:assert/strict';
import test from 'node:test';

import { makeTime } from '../lib/time.js';
import {
  bannersOf, cardsOf, diskHref, diskNamed, findDisk, makeScrubs, needsOf, projectionOf, rowsOf, scrubbingWords, volumesOfDisk,
} from '../pages/storage/model.js';
import {
  data, disk, GB, NOW, partition, passed, ROSE, show, showAlone, showBehind, showConverting, showDup, showFailing,
  showMissing, showSpread, showSystemAndData, showWearing, system, usage30d, volume,
} from './storage-fixtures.mjs';

const time = makeTime('UTC', () => NOW * 1000);

// ---- the banner ----------------------------------------------------------

test('a machine in good order has no banner', () => {
  assert.deepEqual(bannersOf(show(), { time }), []);
  assert.deepEqual(bannersOf(null, { time }), []);
});

test('a mirror that lost a disk says every file is still there and offers the steps to replace it', () => {
  const [banner] = bannersOf(showMissing(), { time });
  assert.equal(banner.tone, 'bad');
  assert.equal(banner.lead, 'The data mirror has lost a disk.');
  assert.equal(banner.detail, 'Every file is still there, on one disk now, with no second copy. Find out which disk is missing, then replace it.');
  assert.deepEqual(banner.buttons, [{ id: 'replace', label: 'How to replace a mirror disk' }]);
});

test('a volume with no second copy that lost a disk does not promise its files', () => {
  const answer = showMissing();
  answer.volumes[1] = data({ devices: ['sda1'], disks: 2, missing: 1, profiles: { data: ['single'], metadata: ['dup'], system: ['dup'] } });
  const [banner] = bannersOf(answer, { time });
  assert.equal(banner.lead, 'The data volume has lost a disk.');
  assert.equal(banner.detail, 'Some files may be gone. Find out which disk is missing, then reconnect or replace it.');
  assert.deepEqual(banner.buttons, []);
});

test('a disk that says it is failing comes after a lost mirror disk and before a count that rose', () => {
  const answer = showFailing();
  answer.disks[1].health = passed({ rose: ROSE, verdict: 'warning', moved: 2 });
  assert.deepEqual(bannersOf(answer, { time }).map((banner) => banner.id), ['failing-sdb', 'rising-sda']);
  const lost = showMissing();
  lost.disks[1].health = passed({ verdict: 'failing' });
  assert.deepEqual(bannersOf(lost, { time }).map((banner) => banner.id), [`missing-${lost.volumes[1].uuid}`, 'failing-sda']);
});

test('a failing disk in a mirror says to copy files off it or replace it while the mirror has a full copy', () => {
  const [banner] = bannersOf(showFailing(), { time });
  assert.equal(banner.lead, 'sdb says it is failing.');
  assert.equal(banner.detail, 'Copy what matters off it now, or replace it while the mirror still has a full copy.');
  assert.deepEqual(banner.buttons.map((button) => button.id), ['open', 'replace']);
});

test('a failing disk that no mirror holds says it may stop at any time', () => {
  const answer = showAlone();
  answer.disks[0].health = passed({ verdict: 'failing' });
  const [banner] = bannersOf(answer, { time });
  assert.equal(banner.detail, 'Copy what matters off it now: it may stop working at any time.');
  assert.deepEqual(banner.buttons.map((button) => button.id), ['open']);
});

test('a count that rose says what moved and when it was seen, and offers a check of that disk', () => {
  const [banner] = bannersOf(showWearing(), { time });
  assert.equal(banner.tone, 'warn');
  assert.equal(banner.lead, 'sdb moved 8 worn-out sectors to spares.');
  assert.equal(banner.detail, 'It still works and the mirror is healthy, but a count that grows is often the first sign a disk is wearing out. Keep backups current and have a replacement ready. Seen 20 hours ago.');
  assert.deepEqual(banner.buttons, [{ id: 'check', label: 'Check sdb now', disk: 'sdb' }, { id: 'replace', label: 'How to replace a mirror disk' }]);
});

test('a count that rose on a disk with no mirror says its files have no second copy', () => {
  const answer = showAlone();
  answer.disks[0].health = passed({ rose: ROSE, verdict: 'warning', moved: 8 });
  const [banner] = bannersOf(answer, { time });
  assert.equal(banner.detail, 'It still works, but a count that grows is often the first sign a disk is wearing out. Its files have no second copy on this machine: keep backups current. Seen 20 hours ago.');
  assert.deepEqual(banner.buttons.map((button) => button.id), ['check']);
});

test('a mirror that is behind needs a check of every block, and is catching up while one runs', () => {
  const answer = showBehind();
  const [banner] = bannersOf(answer, { time });
  assert.equal(banner.tone, 'info');
  assert.equal(banner.lead, 'The data mirror is behind.');
  assert.match(banner.detail, /missed changes while it was away/);
  assert.deepEqual(banner.buttons, [{ id: 'scrub', label: 'Scrub the mirror', mount: '/volumes/data' }]);
  const [running] = bannersOf(answer, { time, scrubbing: ['/volumes/data'] });
  assert.equal(running.lead, 'The data mirror is catching up.');
  assert.equal(running.detail, 'Checking every block is copying the changes that a disk missed.');
});

test('a volume that lost a disk is not also behind', () => {
  const answer = showMissing();
  answer.volumes[1] = { ...answer.volumes[1], behind: 1 };
  assert.deepEqual(bannersOf(answer, { time }).map((banner) => banner.id.split('-')[0]), ['missing']);
});

// ---- the cards ---------------------------------------------------------------

test('the system and the data are a card each, the system first', () => {
  const cards = cardsOf(show(), null);
  assert.deepEqual(cards.map((card) => card.title), ['System', 'Data']);
  assert.deepEqual(cards[0], {
    id: `card-${system().uuid}`, title: 'System', used: '61 GB used', of: 'of 255 GB on nvme0', percent: 24, tone: '', pill: null, note: null, projection: null,
  });
  assert.deepEqual(cards[1], {
    id: `card-${data().uuid}`, title: 'Data', used: '1.24 TB used', of: 'of 2 TB on sda and sdb, each a full copy', percent: 62, tone: '',
    pill: { word: 'Mirror healthy', tone: 'ok' }, note: null, projection: null,
  });
});

test('a system volume that holds the people\'s folders is the system and data', () => {
  assert.deepEqual(cardsOf(showSystemAndData(), null).map((card) => card.title), ['System and data']);
});

test('a volume that is neither is named by its label, and a filesystem that is not Btrfs gets a card but the start-up partition does not', () => {
  const answer = show();
  answer.volumes.push(volume('cccc', 'Photos', '/volumes/photos', ['sdd1']));
  answer.filesystems.push({ mount: '/volumes/backup', type: 'vfat', device: 'sdc1', size: 4e12, used: 1e12, free: 3e12 });
  const titles = cardsOf(answer, null).map((card) => card.title);
  assert.deepEqual(titles, ['System', 'Data', 'Photos', '/volumes/backup']);
  assert.equal(cardsOf(answer, null).find((card) => card.title === '/volumes/backup').of, 'of 4 TB');
});

test('the mirror\'s state is the pill, with its sentence under it', () => {
  const missing = cardsOf(showMissing(), null)[1];
  assert.deepEqual(missing.pill, { word: 'Mirror missing a disk', tone: 'bad' });
  assert.equal(missing.note, '1 of 2 disks is missing. Every file is still there, with no second copy.');
  assert.equal(missing.of, 'of 2 TB on sda');
  assert.equal(cardsOf(showBehind(), null)[1].pill.word, 'Mirror behind');
  assert.equal(cardsOf(showBehind(), null, { scrubbing: ['/volumes/data'] })[1].pill.word, 'Mirror catching up');
  assert.equal(cardsOf(showSpread(), null)[1].pill.word, 'Partly mirrored');
  assert.equal(cardsOf(showConverting(), null)[1].pill.word, 'Changing layout');
  assert.equal(cardsOf(showDup(), null)[0].pill.word, 'Two copies on one disk');
});

test('a card is warm at 90 percent full and bad at 97', () => {
  const full = (used) => {
    const answer = show();
    answer.volumes[1] = data({ used, size: 100 * GB });
    return cardsOf(answer, null)[1];
  };
  assert.equal(full(89 * GB).tone, '');
  assert.equal(full(90 * GB).tone, 'warn');
  assert.equal(full(96 * GB).tone, 'warn');
  assert.equal(full(97 * GB).tone, 'bad');
});

test('the projection is the straight line through 30 days of use, in words', () => {
  const answer = show();
  const found = projectionOf(answer.volumes[1], answer.disks, usage30d({ perDay: 3e9, days: 31 }));
  assert.equal(found.words, 'Full in about 8 months at the rate of the last 30 days');
  assert.equal(cardsOf(answer, usage30d({ perDay: 3e9, days: 31 }))[1].projection, found.words);
});

test('a machine with a few days of history says so, one whose use falls says nothing, and one with no history says so too', () => {
  const answer = show();
  assert.equal(cardsOf(answer, usage30d({ days: 5 }))[1].projection, 'Not enough history yet');
  assert.equal(cardsOf(answer, usage30d({ perDay: -3e9 }))[1].projection, null);
  assert.equal(cardsOf(answer, [])[1].projection, 'Not enough history yet');
  assert.equal(cardsOf(answer, null)[1].projection, null);
});

test('the projection reads the series of the mount of the volume, or of one of its partitions', () => {
  const answer = show();
  const points = usage30d({ perDay: 3e9 });
  const elsewhere = points.map((one) => ({ ...one, series: one.series.replace('/volumes/data', '/users') }));
  assert.ok(projectionOf(answer.volumes[1], answer.disks, elsewhere).seconds > 0);
  assert.equal(projectionOf(answer.volumes[0], answer.disks, points).words, 'Not enough history yet');
});

test('a card whose volume fills within 30 days is warm whatever its percent', () => {
  const answer = show();
  const card = cardsOf(answer, usage30d({ perDay: 35e9 }))[1];
  assert.equal(card.tone, 'warn');
  assert.equal(card.projection, 'Full in about 3 weeks at the rate of the last 30 days');
});

// ---- the disks to scale ----------------------------------------------------------------

test('each disk is a row with its model, size and connection', () => {
  const rows = rowsOf(show());
  assert.deepEqual(rows.map((row) => row.line), ['Samsung PM981 · 256 GB', 'WD Red Plus WD20EFZX · 2 TB', 'WD Red Plus WD20EFZX · 2 TB', 'Seagate Expansion · 4 TB · USB']);
  assert.deepEqual(rows.map((row) => row.href), ['/storage/nvme0', '/storage/sda', '/storage/sdb', '/storage/sdc']);
  assert.equal(diskHref('a b'), '/storage/a%20b');
});

test('a bar is its partitions, named by what they are for, and the room after them', () => {
  const [system, mirror, , stick] = rowsOf(show());
  assert.deepEqual(system.parts.map((part) => [part.label, part.kind]), [['ESP', 'p4'], ['System · 61 GB used', 'p1']]);
  assert.deepEqual(mirror.parts.map((part) => [part.label, part.kind]), [['Data mirror · 1.24 TB used', 'p2']]);
  assert.deepEqual(stick.parts.map((part) => [part.label, part.kind]), [['Not used yet · 4 TB', 'free']]);
  assert.equal(stick.empty, true);
  assert.equal(mirror.empty, false);
});

test('a bar is named in words for a screen reader, with the size of each part', () => {
  const [, mirror, , stick] = rowsOf(show());
  assert.equal(mirror.label, 'sda, 2 TB: Data mirror, 1.24 TB used (2 TB)');
  assert.equal(stick.label, 'sdc, 4 TB: Not used yet, 4 TB (4 TB)');
});

test('a part too narrow for words has a number, and the key says what it is', () => {
  const [system] = rowsOf(show());
  assert.deepEqual(system.parts.map((part) => part.number), [1, null]);
  assert.deepEqual(system.key, ['1 ESP']);
});

test('a disk\'s health is its pill', () => {
  const rows = rowsOf(showWearing());
  assert.deepEqual(rows.map((row) => row.health.word), ['Healthy', 'Healthy', 'Wearing', 'Not checked']);
});

test('other filesystems and unused partitions are named plainly', () => {
  const answer = show();
  answer.disks.push(disk('sdd', 'Stick', 8 * GB, [
    partition('sdd1', 1, 1_048_576, 4 * GB, { filesystem: 'vfat', label: 'PHOTOS' }),
    partition('sdd2', 2, 5 * GB, 3 * GB, { type: 'linux-raid' }),
    partition('sdd3', 3, 8 * GB - 1e6, 1e5, { type: 'swap', filesystem: 'swap' }),
  ], { free_after: 0 }));
  const parts = rowsOf(answer).at(-1).parts;
  assert.deepEqual(parts.map((part) => [part.label, part.kind]), [['PHOTOS', 'p3'], ['Unused', 'free'], ['Swap', 'p4']]);
});

test('a disk is found by name, and the address after /storage/ is its name', () => {
  assert.equal(findDisk(show(), 'sdb').name, 'sdb');
  assert.equal(findDisk(show(), 'sdz'), null);
  assert.equal(diskNamed(''), null);
  assert.equal(diskNamed('/'), null);
  assert.equal(diskNamed('sdb/'), 'sdb');
  assert.equal(diskNamed('a%20b'), 'a b');
  assert.equal(diskNamed('%E0%A4%A'), '%E0%A4%A');
  assert.deepEqual(volumesOfDisk(findDisk(show(), 'sdb'), show()).map((one) => one.label), ['data']);
});

// ---- a scrub as a task --------------------------------------------------------------------

test('a scrub that runs is known from the list, its percent from the events, and its end from the last event', () => {
  const scrubs = makeScrubs();
  scrubs.learn([
    { id: 4, action: 'storage.scrub', state: 'running', percent: 41, args: { mount: '/volumes/data' } },
    { id: 5, action: 'services.restart', state: 'running', args: { name: 'nginx' } },
    { id: 6, action: 'storage.scrub', state: 'done', args: { mount: '/' } },
  ]);
  assert.deepEqual(scrubs.mounts(), ['/volumes/data']);
  assert.deepEqual(scrubs.of('/volumes/data'), { id: 4, mount: '/volumes/data', percent: 41 });
  assert.equal(scrubs.of('/'), null);
  assert.deepEqual(scrubs.event({ id: 4, action: 'storage.scrub', state: 'running', percent: 42 }), {});
  assert.equal(scrubs.of('/volumes/data').percent, 42);
  assert.deepEqual(scrubs.event({ id: 4, action: 'storage.scrub', state: 'running' }), {});
  assert.equal(scrubs.of('/volumes/data').percent, 42, 'a step with no percent keeps the last one');
  assert.deepEqual(scrubs.event({ id: 4, action: 'storage.scrub', state: 'done' }), { ended: '/volumes/data', state: 'done' });
  assert.deepEqual(scrubs.mounts(), []);
});

test('an event for a scrub not known is read whole, and another action\'s event is not a scrub\'s', () => {
  const scrubs = makeScrubs();
  assert.deepEqual(scrubs.event({ id: 9, action: 'storage.scrub', state: 'running', percent: 3 }), { learn: 9 });
  scrubs.put({ id: 9, action: 'storage.scrub', state: 'running', percent: 3, args: { mount: '/' } });
  assert.deepEqual(scrubs.of('/'), { id: 9, mount: '/', percent: 3 });
  assert.deepEqual(scrubs.event({ id: 10, action: 'services.restart', state: 'running' }), {});
  assert.deepEqual(scrubs.event({ id: 77, action: 'storage.scrub', state: 'failed' }), {});
  assert.deepEqual(scrubs.event({ id: 9, action: 'storage.scrub', state: 'failed' }), { ended: '/', state: 'failed' });
  scrubs.started(11, '/volumes/data');
  assert.deepEqual(scrubs.of('/volumes/data'), { id: 11, mount: '/volumes/data', percent: null });
});

test('the button of a scrub that runs says how far it is', () => {
  assert.equal(scrubbingWords({ percent: 41 }), 'Scrubbing… 41%');
  assert.equal(scrubbingWords({ percent: null }), 'Scrubbing…');
  assert.equal(scrubbingWords({ percent: 0 }), 'Scrubbing… 0%');
});

// ---- the Overview's needs -------------------------------------------------------------------

test('a machine in good order gives no need', () => {
  assert.deepEqual(needsOf(show(), usage30d({ perDay: 1e9 })), []);
  assert.deepEqual(needsOf(null, null), []);
});

test('a volume that lost a disk is a need that covers the early warning of the same', () => {
  const [need] = needsOf(showMissing(), null);
  assert.deepEqual(need, {
    id: `storage-missing-${data().uuid}`, severity: 'bad', title: 'The data mirror has lost a disk',
    detail: 'Every file is still there, with no second copy. Replace the missing disk.', href: '/storage', icon: 'storage', at: null, covers: ['MET-103'],
  });
});

test('a disk that says it is failing is a need that links to its page', () => {
  const [need] = needsOf(showFailing(), null);
  assert.deepEqual(need, {
    id: 'storage-failing-SDB-SERIAL', severity: 'bad', title: 'sdb says it is failing',
    detail: 'Copy what matters off it now, or replace it while the mirror still has a full copy.', href: '/storage/sdb', icon: 'storage',
    at: 1_763_120_000, covers: ['HW-111'],
  });
});

test('a count that rose is a warning with the time it was seen', () => {
  const [need] = needsOf(showWearing(), null);
  assert.deepEqual(need, {
    id: 'storage-rising-SDB-SERIAL', severity: 'warn', title: 'sdb is wearing: 8 sectors moved to spares',
    detail: 'sdb moved 8 worn-out sectors to spares. Keep backups current and have a replacement ready.', href: '/storage/sdb', icon: 'storage',
    at: Math.floor(ROSE.at_ms / 1000), covers: ['HW-115'],
  });
});

test('a disk that is failing and has a count that rose is one need, the worse', () => {
  const answer = showFailing();
  answer.disks[2].health.rose = ROSE;
  assert.deepEqual(needsOf(answer, null).map((need) => need.id), ['storage-failing-SDB-SERIAL']);
});

test('a volume that is full within 30 days is a need, in the words of the projection', () => {
  const [need] = needsOf(show(), usage30d({ perDay: 35e9 }));
  assert.equal(need.severity, 'warn');
  assert.equal(need.title, 'Data is full in about 3 weeks');
  assert.equal(need.detail, '1.24 TB of 2 TB is used. At the rate of the last 30 days, it is full in about 3 weeks. Free some space or add a disk.');
  assert.deepEqual(need.covers, ['MET-102']);
  assert.equal(need.href, '/storage');
});

test('a volume that is nearly full also covers the warning that it is', () => {
  const answer = show();
  answer.volumes[1] = data({ used: 1.85e12 });
  const [need] = needsOf(answer, usage30d({ used: 1.85e12, perDay: 25e9 }));
  assert.deepEqual(need.covers, ['MET-101', 'MET-102']);
});

test('a volume that is full at this rate in more than 30 days, or has too little history, is not a need', () => {
  assert.deepEqual(needsOf(show(), usage30d({ perDay: 10e9 })), []);
  assert.deepEqual(needsOf(show(), usage30d({ perDay: 25e9, days: 5 })), []);
  assert.deepEqual(needsOf(show(), usage30d({ perDay: -1e9 })), []);
});

test('a volume that is full now says now', () => {
  const answer = show();
  answer.volumes[1] = data({ used: 2.0e12, size: 2_000_397_795_328 });
  const [need] = needsOf(answer, usage30d({ used: 2_000_397_795_328, perDay: 1e9 }));
  assert.equal(need.title, 'Data is full now');
});
