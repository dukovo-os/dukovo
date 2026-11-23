// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The steps to use a new disk and to replace a mirror's disk (design 4.3 and
// section 10): a dialog with the commands, which the console does not run.
// Each can erase a disk with one wrong name, so the page names the commands
// and lets a person type them at a terminal. The page reads this file when
// one of its buttons is first pressed.

import { h, icon, uniqueId } from '../../dom.js';
import { COPIED, copyText, PRESS_CTRL_C } from '../../lib/copy.js';
import { head, openModal } from '../../shell/dialogs.js';
import { stepsOf } from './disk-detail.js';
import { replaceSteps as raidReplaceSteps } from './raid-model.js';

/** A command in a box with a button that copies it. */
function code(command) {
  const id = uniqueId('sto-code');
  const pre = h('pre', { id, class: 'cmdtext', tabindex: '0' }, command);
  const status = h('span', { class: 'small', role: 'status' });
  const copy = h('button', { class: 'btn small', type: 'button', 'aria-describedby': id }, icon('copy'), 'Copy');
  copy.addEventListener('click', async () => {
    status.textContent = (await copyText(command, pre)) ? COPIED : PRESS_CTRL_C;
  });
  return h('div', { class: 'cmd' }, pre, h('div', { class: 'btns' }, copy, status));
}

/** A dialog named by `title`, with `body` and one button to close it. */
function dialog(title, body) {
  const id = uniqueId('sto-steps');
  return h('dialog', { class: 'dialog', 'aria-labelledby': id },
    h('form', { method: 'dialog' },
      head(id, 'info', title),
      h('div', { class: 'dialog-body sto-steps' }, ...body),
      h('div', { class: 'dialog-foot' }, h('button', { class: 'btn default', type: 'submit', value: 'close' }, 'Close'))));
}

/** The steps to replace a disk of a mirror at `mount`, with `survivor` the disk that is left (null when it is not known). */
function replaceSteps(mount, survivor) {
  return [
    h('p', {}, 'The console does not rebuild a disk for you: each step can erase a disk if the wrong one is named. Run these at a terminal, as an administrator.'),
    h('ol', { class: 'sto-steps-list' },
      h('li', {}, h('p', {}, 'Look after the disk that is left. While one disk is missing or failing, the other holds the only full copy. Copy what matters off it if you can.')),
      h('li', {}, h('p', {}, 'Put the new disk in. It shows on this page as Not used yet, and it must be as big as the disk it replaces, or bigger.')),
      h('li', {}, h('p', {}, `Give it partitions like the old disk's. The first command shows the table of ${survivor ?? 'the disk that is left'}; the second makes the new disk's (NEW is its name here, such as sdd).`),
        code(`elevate rdisk -l ${survivor ?? 'DISK'}\nelevate rdisk NEW`)),
      h('li', {}, h('p', {}, 'Rebuild onto the new partition, NEWPART, which takes the old one\'s place, such as sdd1. DEVID is the missing disk\'s device ID, 1 or 2; a disk that is still there can be named by its device instead, such as /devices/sdb1.'),
        code(`elevate replace start DEVID /devices/NEWPART ${mount}`)),
      h('li', {}, h('p', {}, 'It prints when the new disk holds everything, and the machine stays usable meanwhile. Then press Scrub the mirror on this page, to check every block.')),
      ...(mount === '/' ? [h('li', {}, h('p', {}, 'This is the system\'s own mirror, so put the start-up code on the new disk too:'), code('elevate rgrub-setup NEW'))] : [])),
    h('p', { class: 'small muted' }, 'The handbook\'s Storage section has the same steps with more about each.'),
  ];
}

/** The steps to use a disk that is new to the machine. */
function newSteps(mount, backups) {
  return [
    h('p', {}, 'Each of these erases what is on the disk it names. Check the name against the list of disks on this page before you type it. NEW is the disk\'s name here, such as sdd, and NEWPART the partition you make on it, such as sdd1.'),
    h('ul', { class: 'sto-steps-list' },
      backups && h('li', {}, h('p', {}, h('b', {}, 'As a place for backups. '), 'Add it on the ', h('a', { href: '/snapshots/backups' }, 'Snapshots and backups page'), '.')),
      h('li', {}, h('p', {}, h('b', {}, 'As a volume of its own. '), 'Make a partition and a Btrfs filesystem on it.'),
        code('elevate rdisk NEW\nelevate mkfs-btrfs -L NAME /devices/NEWPART')),
      h('li', {}, h('p', {}, h('b', {}, 'As a second disk, to make a mirror. '), 'Add it to the volume and turn its data and metadata into RAID1.'),
        code(`elevate rdisk NEW\nelevate device add /devices/NEWPART ${mount}\nelevate balance start -dconvert=raid1 -mconvert=raid1 ${mount}`))),
    h('p', { class: 'small muted' }, 'The handbook\'s Storage section has the same steps with more about each.'),
  ];
}

/** The steps to replace the failed disk of a RAID volume: words only, since the controller does the copying. */
function raidSteps(raid) {
  const { intro, steps } = raidReplaceSteps(raid);
  return [
    h('p', {}, intro),
    h('ol', { class: 'sto-steps-list' }, steps.map((words) => h('li', {}, h('p', {}, words)))),
    h('p', { class: 'small muted' }, 'The handbook\'s Storage section has the same steps with more about each.'),
  ];
}

/**
 * Opens the steps `kind` (`replace` or `new`). `answer` is `storage show`'s
 * and `disk` the name of the disk whose page is open, or null; `backups` says
 * whether the Snapshots page exists. `raid` (`{bays, volume}`, from
 * raid-model.js) makes the replace steps those of a failed disk in a
 * controller's bay, in place of a mirror's. Resolves when the dialog closes.
 */
export function openSteps(kind, { answer, disk = null, backups = false, raid = null }) {
  if (kind === 'replace' && raid) return openModal(dialog('Replace a failed disk', raidSteps(raid)));
  const { mount, survivor } = stepsOf(answer, disk);
  if (kind === 'new') return openModal(dialog('Use a new disk', newSteps(mount, backups)));
  return openModal(dialog('Replace a mirror disk', replaceSteps(mount, survivor)));
}
