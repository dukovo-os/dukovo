// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// What the Move to new hardware page says and decides (design 6), as plain
// values, so that a test reaches every branch: what the address asks for, the
// backups and key files to choose from, the arguments that `move.plan` and
// `move.stage` take, the choice of a network card's address, and the words of
// what comes across and what differs. `found` is `GET /api/1/move/sources`
// (`backup find --json`), `plan` is `POST /api/1/move/plan` (`backup restore
// --machine --plan --json`) and `backups` is `GET /api/1/backups`; see section
// 11.6 of the design. The files of this folder only draw what these give.
// A typed recovery key is never an argument of anything here except
// `argsOf`, which builds one call from it and keeps nothing.

import { listWords } from '../../lib/disk-model.js';
import { bytesWords, counted } from '../../lib/size-words.js';
import { whenWords } from '../../lib/snapshot-words.js';

/** The list `name` of `answer`, or an empty one when the answer has none. */
const listOf = (answer, name) => (Array.isArray(answer?.[name]) ? answer[name] : []);

/** `words` as a sentence. */
const sentence = (words) => (/[.!?]$/.test(words) ? words : `${words}.`);

/** An address without its prefix length: "192.168.1.12/24" is "192.168.1.12". */
const plain = (address) => String(address).split('/')[0];

/** The last part of a path. */
const baseName = (path) => path.slice(path.lastIndexOf('/') + 1);

/** The volume under /volumes that `path` is on, or null. */
const volumeOf = (path) => /^\/volumes\/([^/]+)/.exec(path)?.[1] ?? null;

// ---- the address -------------------------------------------------------------

/** A backup place's name, as backup(8) takes one. */
const PLACE_NAME = /^[A-Za-z0-9_-]{1,32}$/;

/** The longest path an action takes. */
const PATH_MOST = 4096;

/** Why `text` is not a folder a machine's backups can be read from, or empty text. */
export function folderFault(text) {
  if (text === '') return 'Type the whole path of the folder.';
  if (!text.startsWith('/')) return 'Write the whole path, starting with /.';
  if (/[\u0000-\u001f\u007f]/.test(text) || text.split('/').includes('..') || new TextEncoder().encode(text).length > PATH_MOST) {
    return 'The backups cannot be read from that path.';
  }
  return '';
}

/**
 * What the address asks for, from its query: `from` (a backup place's name)
 * or `source` (a folder of a machine's backups) chooses step 1, and `via`
 * says the person came from Finish setting up. A value that is not what it
 * names is left out.
 */
export function entryOf(query) {
  const from = query.get('from') ?? '';
  const source = query.get('source') ?? '';
  return {
    from: PLACE_NAME.test(from) ? from : null,
    source: source !== '' && folderFault(source) === '' ? source : null,
    setup: query.get('via') === 'setup',
  };
}

/** The crumb of the head: the page that the person came from. */
export const crumbsOf = (entry) => [entry.setup
  ? { title: 'Finish setting up', path: '/setup' }
  : { title: 'Snapshots and backups', path: '/snapshots' }];

// ---- the backups to choose from, and the keys ------------------------------------

/**
 * The backups of step 1, `{disks, places}`, each `{id, kind, source, machine,
 * title, detail}`. `disks` are those `backup find` found on plugged-in
 * disks, "USB disk BACKUP" with "duke-nas, today 1:30 AM"; `places` are the
 * backup places of this machine, which a machine that was just installed has
 * none of. `source` is what the actions take: a folder, or a place's name.
 * `time` is lib/time.js's.
 */
export function sourcesOf(found, backups, time) {
  const disks = listOf(found, 'sources').filter((one) => typeof one?.source === 'string').map((one) => ({
    id: `disk:${one.source}`,
    kind: 'disk',
    source: one.source,
    machine: one.machine ?? null,
    title: `${one.removable ? 'USB disk' : 'Disk'} ${one.volume ?? volumeOf(one.source) ?? ''}`.trim(),
    detail: [one.machine, Number.isFinite(one.newest_ms) && one.newest_ms > 0 ? whenWords(one.newest_ms, time) : null].filter(Boolean).join(', '),
  }));
  const places = listOf(backups, 'places').filter((place) => typeof place?.name === 'string').map((place) => ({
    id: `place:${place.name}`,
    kind: 'place',
    source: place.name,
    machine: null,
    title: `Backup place ${place.name}`,
    detail: typeof place.to === 'string' ? place.to : '',
  }));
  return { disks, places };
}

/** The backup that the address chose, among `items` (disks and places in one list), or null. */
export function chosenByEntry(items, entry) {
  if (entry.from !== null) return items.find((item) => item.kind === 'place' && item.source === entry.from) ?? null;
  if (entry.source !== null) return items.find((item) => item.kind === 'disk' && item.source === entry.source) ?? null;
  return null;
}

/** The key files that `backup find` found, `[{id, path, name, for, title}]`: "Recovery key from the KEYS stick". */
export const keysOf = (found) => listOf(found, 'keys').filter((key) => typeof key?.path === 'string').map((key) => ({
  id: `key:${key.path}`,
  path: key.path,
  name: baseName(key.path),
  for: typeof key.for === 'string' ? key.for : null,
  title: volumeOf(key.path) ? `Recovery key from the ${volumeOf(key.path)} stick` : `Recovery key ${baseName(key.path)}`,
}));

/**
 * The key file that is chosen when step 2 opens: one named after the
 * machine of the chosen backup (`duke-nas-recovery.key`), or made for the
 * backup place that was chosen. Null when none is, so that nothing is
 * chosen for a person who has to look.
 */
export function defaultKey(keys, source) {
  const names = [source?.machine, source?.kind === 'place' ? source.source : null].filter(Boolean).map((name) => name.toLowerCase());
  return keys.find((key) => names.some((name) => [`${name}-recovery.key`, `${name}.key`].includes(key.name.toLowerCase()) || key.for?.toLowerCase() === name)) ?? null;
}

// ---- what the actions take -------------------------------------------------------------

/**
 * The arguments of `move.plan` and `move.stage`: the backup, and exactly one
 * of the key file and the typed key, which wins when both are given; the
 * lines of the cards' choices and the time of the backup when they are
 * given. This is the one place a typed key is put in a call's arguments.
 */
export function argsOf({
  source, file = null, typed = '', cards = '', at = null,
}) {
  const args = { source };
  if (typed !== '') args.key = typed;
  else if (file !== null) args.key_file = file;
  if (cards !== '') args.cards = cards;
  if (at !== null) args.at = at;
  return args;
}

/**
 * The arguments of `POST /api/1/show` for the call that `argsOf` builds, with
 * the key as the file's path (`how` is `file`) or as an empty text (`how` is
 * `typed`): the command says "then the key, typed when asked", and no typed
 * key is ever sent there.
 */
export function showArgs({
  source, how, file = null, cards = '', at = null,
}) {
  const args = argsOf({ source, file: how === 'file' ? file : null, cards, at });
  return how === 'typed' ? { ...args, key: '' } : args;
}

// ---- the network cards' choices --------------------------------------------------------

/** What an old card's address is given in place of a new card's. */
export const AUTOMATIC = 'automatic';

/** The most `--card` lines an action takes. */
const MOST_CARDS = 8;

/** The old cards of `plan` with fixed addresses, which the page offers a choice for. */
export const cardsOf = (plan) => listOf(plan, 'cards').filter((card) => typeof card?.old?.mac === 'string' && card.old.mac !== '').slice(0, MOST_CARDS);

/** The choice for each old card when nothing was chosen: the new card the plan pairs it with, else automatic. */
export function defaultPicks(plan) {
  return Object.fromEntries(cardsOf(plan).map((card) => [card.old.mac, typeof card.new?.mac === 'string' ? card.new.mac : AUTOMATIC]));
}

/** The choice for `card`: a new card's MAC or `AUTOMATIC`. */
const pickOf = (picks, card) => picks?.[card.old.mac] ?? AUTOMATIC;

/** The cards of this machine that `card` may be given: all of them when the plan knows the hardware, else the one it paired. */
function candidatesOf(plan, card) {
  if (plan?.hardware != null) return listOf(plan.hardware.now, 'cards').filter((one) => typeof one?.mac === 'string');
  return typeof card.new?.mac === 'string' ? [card.new] : [];
}

/** The lines of the `cards` argument: `OLD=NEW`, or `OLD=automatic`, for each old card. */
export const cardLines = (plan, picks) => cardsOf(plan).map((card) => `${card.old.mac}=${pickOf(picks, card)}`).join('\n');

/** How many old cards change the staged network settings: a card given another's MAC, or left to ask for its address. */
export const changesOf = (plan, picks) => cardsOf(plan).filter((card) => pickOf(picks, card) !== card.old.mac).length;

/** The addresses of the old machine that the choices keep, each without its prefix length. */
export function keptAddresses(plan, picks) {
  return cardsOf(plan).filter((card) => pickOf(picks, card) !== AUTOMATIC).flatMap((card) => listOf(card.old, 'addresses').map(plain));
}

/** The words that say two old cards were given one new card, or empty text when none was. */
export function clashWords(plan, picks) {
  const taken = new Map();
  for (const card of cardsOf(plan)) {
    const pick = pickOf(picks, card);
    if (pick !== AUTOMATIC) taken.set(pick, [...(taken.get(pick) ?? []), card.old.name ?? card.old.mac]);
  }
  const twice = [...taken.values()].find((names) => names.length > 1);
  return twice ? `${listWords(twice)} cannot both take the same port. Let one of them get an address automatically.` : '';
}

/** The words of a new card: "net0 (Realtek RTL8111)". */
const cardWords = (card) => (card.model ? `${card.name} (${card.model})` : card.name);

/** The addresses of an old card, as a sentence says them. */
const addressWords = (card) => listOf(card.old, 'addresses').map(plain).join(', ');

/**
 * The choices of each old card that has a new card to take its address, `[{mac,
 * label, options: [{value, words, checked}]}]`: the old machine's address
 * fixed on a card, for each card of this machine it may go to, or an address
 * asked for automatically.
 */
export function choiceGroups(plan, picks, machine) {
  return cardsOf(plan).map((card) => {
    const candidates = candidatesOf(plan, card);
    const whose = `${machine}'s address, ${addressWords(card)}, fixed`;
    const options = [
      ...candidates.map((one) => ({ value: one.mac, words: candidates.length === 1 ? `Give it ${whose}` : `Give ${cardWords(one)} ${whose}` })),
      { value: AUTOMATIC, words: 'Let it get an address automatically' },
    ].map((option) => ({ ...option, checked: option.value === pickOf(picks, card) }));
    return { mac: card.old.mac, label: `${card.old.name ?? card.old.mac}'s address`, hasChoice: candidates.length > 0, options };
  });
}

// ---- the space -----------------------------------------------------------------------------

/**
 * Whether the files fit: `{known, fits, needed, free, spare, words}`. `words`
 * is the sentence that says why they do not, "The files need 1.24 TB and this
 * machine has 0.9 TB free", and empty text when they do or the plan does not
 * say.
 */
export function spaceOf(plan) {
  const needed = Number.isFinite(plan?.needed) ? plan.needed : null;
  const free = Number.isFinite(plan?.free) ? plan.free : null;
  const known = needed !== null && free !== null;
  const fits = typeof plan?.fits === 'boolean' ? plan.fits : !known || needed <= free;
  return {
    known,
    fits,
    needed,
    free,
    spare: known ? free - needed : null,
    words: known && !fits ? sentence(`The files need ${bytesWords(needed)} and this machine has ${bytesWords(free)} free`) : '',
  };
}

// ---- what comes across ---------------------------------------------------------------

/** The places of a record that are not people's files: the settings and the apps, which have rows of their own. */
const SYSTEM_PLACES = Object.freeze(['/system/settings', '/applications']);

/** Whether `place` is on a disk of its own, under /volumes, which a restore of the machine does not put in place. */
const onDisk = (place) => typeof place?.path === 'string' && place.path.startsWith('/volumes/');

/** The words of the accounts of `plan`: their names, none, or "its accounts" for a record that does not name them (format 1). */
function accountsWords(plan) {
  if (!Array.isArray(plan.accounts)) return 'its accounts, with their passkeys and SSH keys';
  return plan.accounts.length > 0 ? `${listWords(plan.accounts)}, with their passkeys and SSH keys` : 'no accounts of people';
}

/** The words that stand for a row that comes across: `{tone, words}`. */
const COMES = Object.freeze({ tone: 'ok', words: 'Comes across' });

/**
 * The rows of "What comes across", `[{id, title, detail, pill, muted}]`:
 * the name and accounts, the settings (with the changes for the network
 * cards that the choices make), the apps, the files, the files of other
 * disks when the record has any, which are restored beside, and the older
 * snapshots, which are not copied. `picks` are the cards' choices.
 */
export function comesAcross(plan, picks) {
  const machine = plan.machine ?? 'the old machine';
  const apps = listOf(plan, 'apps');
  const places = listOf(plan, 'places').filter((place) => !SYSTEM_PLACES.includes(place.path));
  const files = places.filter((place) => !onDisk(place));
  const disks = places.filter(onDisk);
  const sized = files.length > 0 && files.every((place) => Number.isFinite(place.bytes));
  const changes = changesOf(plan, picks);
  const { fits } = spaceOf(plan);
  return [
    {
      id: 'names',
      title: 'Name, accounts and passwords',
      detail: `${machine} · ${accountsWords(plan)}`,
      pill: COMES,
    },
    {
      id: 'settings',
      title: 'Settings',
      detail: `/system/settings${changes > 0 ? `, with ${counted(changes, 'change')} for the new network ${changes === 1 ? 'port' : 'ports'} (below)` : ''}`,
      pill: COMES,
    },
    {
      id: 'apps',
      title: 'Apps',
      detail: apps.length > 0 ? `${counted(apps.length, 'app')} at the same versions, each with the permissions it was given` : 'No apps are installed',
      pill: COMES,
    },
    {
      id: 'files',
      title: 'Files',
      detail: `${listWords(files.map((place) => place.path)) || 'No folders'}${sized ? `, ${bytesWords(files.reduce((sum, place) => sum + place.bytes, 0))}` : ''}`,
      pill: fits ? COMES : { tone: 'bad', words: 'Does not fit' },
    },
    disks.length > 0 && {
      id: 'other-disks',
      title: 'Files on other disks',
      detail: `${listWords(disks.map((place) => place.path))}: each is restored as a snapshot on this machine's disk of the same name, when it has one, and otherwise stays in the backups`,
      pill: null,
      muted: 'Restored beside',
    },
    {
      id: 'snapshots',
      title: 'Older snapshots',
      detail: 'They stay in the backups; this machine starts its own',
      pill: null,
      muted: 'Not copied',
    },
  ].filter(Boolean);
}

// ---- what differs in the hardware -------------------------------------------------

/** Why the hardware's rows are missing from a record made before Dukovo kept the hardware. */
export const FORMAT_ONE_WORDS = 'These backups were made before Dukovo kept the machine\'s hardware with them, so the differences cannot be shown.';

/** The numbers of one to ten in words, which a sentence of disks reads better with. */
const NUMBERS = ['no', 'one', 'two', 'three', 'four', 'five', 'six', 'seven', 'eight', 'nine', 'ten'];

/** "two 2 TB disks", "one 2 TB disk", or "3 disks of 2 TB, 1 TB and 1 TB" when their sizes differ. */
function disksWords(disks) {
  const count = disks.length;
  const number = NUMBERS[count] ?? String(count);
  const sizes = disks.map((disk) => bytesWords(disk.size));
  if (count === 0) return 'no disk';
  if (new Set(sizes).size === 1) return `${number} ${sizes[0]} ${count === 1 ? 'disk' : 'disks'}`;
  return `${number} disks of ${listWords(sizes)}`;
}

/** The network ports row: the old cards, this machine's, and what each fixed address may become. */
function portsRow(plan, picks, machine) {
  const hardware = plan.hardware;
  const cards = cardsOf(plan);
  const groups = choiceGroups(plan, picks, machine).filter((group) => group.hasChoice);
  const gone = cards.filter((card) => candidatesOf(plan, card).length === 0).map((card) => card.old.name ?? card.old.mac);
  const lines = hardware == null
    ? [`${machine} had fixed addresses on ${listWords(cards.map((card) => card.old.name ?? card.old.mac))}.`]
    : [`${machine} had ${listWords(listOf(hardware.then, 'cards').map(cardWords)) || 'no network port'}. This machine has ${listWords(listOf(hardware.now, 'cards').map(cardWords)) || 'no network port'}.`];
  const notes = [
    gone.length > 0 && `${listWords(gone)}'s settings are kept but unused.`,
    ...listOf(plan, 'firewall_on_missing').map((rule) => `The firewall rule "${rule}" names a port this machine does not have, so it does nothing here.`),
  ].filter(Boolean);
  return {
    id: 'ports',
    title: 'Network ports',
    pill: groups.length > 0 ? { tone: 'info', words: counted(groups.length, 'choice') } : { tone: 'ok', words: 'Nothing to choose' },
    lines,
    notes,
    groups,
    clash: clashWords(plan, picks),
  };
}

/** The disks row: what the old machine kept its data on, what this machine has, and whether the files fit. */
function disksRow(plan, machine, links) {
  const { then, now } = plan.hardware;
  const space = spaceOf(plan);
  const unmirrored = then.mirror === true && now.mirror !== true;
  const kept = then.mirror ? `a mirror of ${disksWords(listOf(then, 'disks'))}` : disksWords(listOf(then, 'disks'));
  const have = listOf(now, 'disks');
  const named = have.map((disk) => (disk.model ? `${disk.model} (${bytesWords(disk.size)})` : bytesWords(disk.size)));
  const here = have.length === 0 ? 'no disk' : `${have.length === 1 ? 'one disk' : `${have.length} disks`}, ${listWords(named)}`;
  const free = space.known ? ` with ${bytesWords(space.free)} free` : '';
  const spare = space.known && space.fits ? `, so the files fit with ${bytesWords(space.spare)} to spare` : '';
  const notes = [
    !space.fits && space.words,
    unmirrored && `Without a mirror, a disk that fails loses the changes since the last backup.${links.storage ? ' Storage shows how to add a disk.' : ''}`,
  ].filter(Boolean);
  let pill = { tone: 'ok', words: 'Fits' };
  if (!space.fits) pill = { tone: 'bad', words: 'Does not fit' };
  else if (unmirrored) pill = { tone: 'warn', words: 'No mirror' };
  return {
    id: 'disks', title: 'Disks', pill, lines: [`${machine} kept its data on ${kept}. This machine has ${here}${free}${spare}.`], notes, groups: [], clash: '',
  };
}

/**
 * The rows of "Differences in the hardware", `{note, rows}`, each row `{id,
 * title, pill: {tone, words}, lines, notes, groups, clash}`. A record of
 * format 1 has no hardware: `note` says so, and the only row is the
 * network ports, from the backup's network settings alone. `links.storage`
 * says whether this console has the Storage page.
 */
export function hardwareOf(plan, picks, links = {}) {
  const machine = plan.machine ?? 'the old machine';
  if (plan.hardware == null) {
    return { note: FORMAT_ONE_WORDS, rows: cardsOf(plan).length > 0 ? [portsRow(plan, picks, machine)] : [] };
  }
  const { then, now } = plan.hardware;
  const rows = [portsRow(plan, picks, machine), disksRow(plan, machine, links)];
  if (then.serial && !now.serial) {
    rows.push({
      id: 'serial',
      title: 'Serial console',
      pill: { tone: 'info', words: 'Plug it in' },
      lines: [`${machine} sent its console to a USB serial adapter${then.serial.model ? `, ${then.serial.model}` : ''}. Plug the same adapter in here and it carries on.`],
      notes: [],
      groups: [],
      clash: '',
    });
  }
  if (then.cpu || now.cpu) {
    rows.push({
      id: 'cpu',
      title: 'Processor and graphics',
      pill: { tone: 'ok', words: 'Nothing to change' },
      lines: [then.cpu && now.cpu && then.cpu !== now.cpu ? `${now.cpu} replaces ${then.cpu}.` : `${now.cpu || then.cpu}, as before.`],
      notes: [],
      groups: [],
      clash: '',
    });
  }
  return { note: '', rows };
}

// ---- after the restart ----------------------------------------------------------------------

/** The command that restarts the machine, until the Hardware page has a button. */
export const RESTART_COMMAND = 'elevate reboot';

/** An address as a URL has it: an IPv6 address is in brackets. */
const hostOf = (address) => (address.includes(':') ? `[${address}]` : address);

/**
 * What the page says of the restart: the banner (bad while the old
 * machine's fixed address is kept, warn otherwise), the three lines of the
 * board, and where to open the console after it, with both addresses when one
 * is kept. `port` is the console's, as it is now.
 */
export function afterOf(plan, picks, { port = '' } = {}) {
  const machine = plan.machine ?? 'the old machine';
  const kept = keptAddresses(plan, picks);
  const at = port ? `:${port}` : '';
  const urls = [`https://${machine}.local${at}/`, ...kept.map((address) => `https://${hostOf(address)}${at}/`)];
  return {
    banner: {
      tone: kept.length > 0 ? 'bad' : 'warn',
      lead: `Turn the old ${machine} off before this machine restarts.`,
      detail: `Otherwise two machines answer as ${machine}${kept.length > 0 ? ` at ${listWords(kept)}` : ''}, and both run the nightly backups.`,
    },
    lines: [
      `The services start with ${machine}'s settings, then the console checks each one and each website`,
      'Anything that fails a check is listed on the Overview, with a way to fix or undo it',
      'This fresh install is kept, read-only, in /snapshots/machine-restore until you remove it',
    ],
    open: `After the restart, open ${urls.join(' or ')} and sign in as one of ${machine}'s administrators.`,
  };
}

// ---- the steps ---------------------------------------------------------------------------------

/** The four steps, in order. */
export const STEPS = Object.freeze([
  { id: 'find', title: 'Find the backup' },
  { id: 'unlock', title: 'Unlock it' },
  { id: 'check', title: 'Check what changes' },
  { id: 'restart', title: 'Restart to finish' },
]);

/**
 * The four steps as the list shows them, `[{id, title, summary, done,
 * reachable}]`. `state` is `{choice, key, plan, reviewed, staged, staging}`: the
 * backup chosen (a `sourcesOf` item), the key as `{how: 'file' | 'typed',
 * title}` once the backups were unlocked, the plan, whether the person went
 * on from it, whether a restore is staged and whether one is being staged
 * (each makes the last step reachable by itself, since cancelling it or
 * watching it is what is left to do). A step is reachable when the ones
 * before it are done.
 */
export function stepsOf({
  choice = null, key = null, plan = null, reviewed = false, staged = false, staging = false,
}) {
  const done = [choice !== null, plan !== null, plan !== null && reviewed, staged];
  const summaries = [
    choice ? [choice.title, choice.detail].filter(Boolean).join(' · ') : '',
    plan && key ? (key.how === 'file' ? key.title : 'Recovery key typed') : '',
    '',
    staged ? 'Staged' : '',
  ];
  return STEPS.map((step, at) => ({
    ...step,
    summary: summaries[at],
    done: done[at],
    reachable: at === 3 && (staged || staging) ? true : done.slice(0, at).every(Boolean),
  }));
}

/** The step to show: `wanted` when it is reachable, else the first that is not done, else the last. */
export function currentStep(steps, wanted) {
  const asked = steps.find((step) => step.id === wanted && step.reachable);
  return (asked ?? steps.find((step) => !step.done && step.reachable) ?? steps[steps.length - 1]).id;
}

// ---- staging ------------------------------------------------------------------------------------

/** What step 4 says after "Staged": nothing changes yet, and the restore can be cancelled. */
export const STAGED_WORDS = 'Nothing changes until this machine restarts, and you can cancel until then.';

/** The line above the steps, for the machine that is being moved when it is known. */
export const leadOf = (machine) => `This machine takes ${machine ? `${machine}'s` : 'the old machine\'s'} name, accounts, settings, apps and files from a backup. Nothing here changes until the restart, and you can cancel until then.`;

/** Why Stage it is off, or empty text: the files do not fit, two cards were given one port, or a restore is staged or being staged. */
export function stageFault(plan, picks, { staged = false, running = false } = {}) {
  if (staged) return 'A restore is staged already. Cancel it to stage again.';
  if (running) return 'A restore is being staged.';
  return spaceOf(plan).words || clashWords(plan, picks);
}
