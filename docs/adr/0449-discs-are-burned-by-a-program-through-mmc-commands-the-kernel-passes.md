<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0449. Discs are burned by a program, through MMC commands the kernel passes

- Status: Accepted
- Date: 2026-11-10

## Context

ADRs 0431 to 0434 and 0447 read discs. Roadmap 11h's last step is to
write them: CD-R and CD-RW, DVD±R and DVD±RW, and BD-R and BD-RE, of
data or of music.

- A drive writes a disc by MMC-6's commands, each kind of disc its own
  way. A CD is written a track at a time (TAO) or all at once from a cue
  sheet (SAO), as the write parameters mode page (05h) says; a CD of
  music is written at once, so that its tracks follow each other with the
  pregaps the cue sheet gives. A DVD-R is written incrementally, a DVD+R
  and a BD-R in sequence and then closed, and a DVD+RW, a DVD-RAM and a
  BD-RE are formatted once and then written anywhere, as a disk is. A
  CD-RW or DVD-RW is blanked to be written again.
- A drive that runs out of data while it writes a CD spoils the disc,
  unless its buffer underrun protection (BUFE in the write parameters
  page) is on.
- Linux leaves all of it to programs: cdrecord, wodim and growisofs send
  the commands raw with SG_IO. FreeBSD's burncd(8) had the ATAPI CD
  driver, acd(4), take requests of its own instead. wodim and growisofs
  are GPL; the commands are written here from MMC-6 itself.
- A disc is written from an image: ISO 9660 with Rock Ridge for POSIX
  names, modes and links, Joliet for Windows' Unicode names, and UDF for
  DVDs and Blu-ray discs and files over 4 GiB. Linux makes them with
  xorriso, genisoimage or mkisofs, all GPL. ECMA-119, ECMA-167 with
  OSTA's UDF 2.01, Microsoft's Joliet specification and IEEE P1281 and
  P1282 (SUSP and RRIP) are free to read.
- A command passed through raw is dangerous: WRITE BUFFER loads firmware,
  and MODE SELECT can save settings in the drive for good.
- QEMU 8.2 has no drive that writes a disc. The tests' own drive, `ob
  disc-drive` (ADR 0447), reads them.

## Decision

- **radix-block(2) has MMC_COMMAND,** Dukovo's own request, as SG_IO is
  Linux's: a 16-byte command block, a buffer of up to 64 KiB, a timeout
  of up to 600 seconds, and an answer of the bytes moved and the sense
  key, ASC and ASCQ of a command that failed. The kernel passes only the
  commands that read what a drive and its disc are, and those that
  write a disc (`radix_scsi::recording::passed`):
  - with the read right: TEST UNIT READY, REQUEST SENSE, INQUIRY, READ
    FORMAT CAPACITIES, READ CAPACITY, READ TOC, GET CONFIGURATION, READ
    DISC INFORMATION, READ TRACK INFORMATION, MODE SENSE(10), READ BUFFER
    CAPACITY and GET PERFORMANCE;
  - with the write right: FORMAT UNIT, WRITE(10) and WRITE(12),
    SYNCHRONIZE CACHE, RESERVE TRACK, MODE SELECT(10), CLOSE
    TRACK/SESSION, SEND CUE SHEET, BLANK, SET STREAMING and SET CD
    SPEED.

  Which way the data goes comes from the command, never from the
  caller. MODE SELECT passes only with its Save Pages bit clear and
  only the write parameters page. Reads of a disc's data, ejects,
  firmware and events do not pass: they go through the kernel's own
  paths.
- **A drive being written is held.** A command that writes holds the
  drive, and so does any command while it is held, for 30 seconds after
  the last. While held, the drive's thread does not look at the disc,
  the tray stays locked, and EJECT is refused as in use but to a program
  with the write right. REREAD_PARTITIONS lets the drive go and returns
  once its thread has looked at the disc afresh, so the disc just burned
  is found as any disc put in is, is in `/devices` when burn(1) ends,
  and volumed(8) mounts it. EJECT from the program burning lets the
  drive go and ejects the disc, before anything mounts it.
- **burn(1) writes discs** (systools). It writes the drive named, or
  the first that holds a recordable disc, asks what the disc is (GET
  CONFIGURATION's current profile, READ DISC INFORMATION, READ TRACK
  INFORMATION, READ FORMAT CAPACITIES), and writes it its way:
  - a CD of data a track at a time, a CD of music at once from a cue
    sheet, a DVD-R and DVD-RW incrementally, a DVD+R, DVD+R DL and BD-R
    in sequence and finalized, and a DVD+RW, DVD-RW in restricted
    overwrite, DVD-RAM or BD-RE formatted if it is not and written from
    block 0;
  - with buffer underrun protection wherever the page has it, and `--test`
    for a write that records nothing where the drive can;
  - a drive busy writing what it holds (NOT READY, long write in
    progress) is waited for, and a write that finds its buffer full is
    sent again, for up to a minute.

  It burns an image, a folder made into an image as it is written, or,
  with `--audio`, WAV and FLAC files, a track each, converted to 44.1 kHz
  stereo, each padded to a whole sector and to four seconds as the Red
  Book wants. `burn blank` blanks a CD-RW or DVD-RW, and `burn info`
  says what disc is in the drive. CD-Text is not written yet.
- **Images are the disk crate's own (`disk::disc`),** written from the
  specifications: ISO 9660 with Rock Ridge 1.10 and Joliet, and with
  `--udf` a UDF 2.01 bridge, the trees pointing at one copy of each
  file's bytes. A folder is scanned first, laid out sector by sector,
  and written reading each file as it comes, so that a folder is burned
  with no image on a disk first. mkiso(1) writes an image of a folder.
- **The emulated drive records** (`tools/xtask/src/disc_drive/record.rs`).
  `ob disc-drive` takes a blank CD-R, CD-RW, DVD-R, DVD+R, DVD+RW, BD-R
  or BD-RE and answers the commands burn sends as MMC-6 says a drive
  does: the write parameters page, the disc's and the track's
  information and status, formats, blanking, the cue sheet, writes at
  the next writable address, and closing. A disc written and closed reads
  back as the tracks written, and a test write leaves it blank.

## Consequences

- The boot test `disc-burn` plugs the emulated drive in with the music CD
  of `audio-cd`:
  - It rips track 1 to FLAC and track 2 to WAV, ejects the disc, and
    burns both files at once onto a blank CD-R. The drive holds the very
    samples ripped, the second track padded with silence to four
    seconds, and `audiocd` lists the two tracks.
  - It burns a folder of a file with a long name in a Unicode-named
    folder, 700,000 bytes of noise and a symbolic link onto a blank CD-R
    a track at a time; the kernel finds the disc, volumed mounts it at
    its label, and each file and the link read back as they were.
  - mkiso makes an image of the folder with UDF; burn formats a blank
    DVD+RW and writes the image onto it, and volumed mounts it as UDF.
  - `burn --eject` writes the image onto a blank CD-R, and the disc comes
    out before anything mounts it.
- Unit tests cover the kernel's list of commands (what passes, with
  which right, which way, and MODE SELECT's page), the images (read back
  by the kernel's own ISO 9660 and UDF readers, and by isoinfo and
  udfinfo where installed), burn's write parameters, cue sheets, padding
  and answers, and the emulated drive writing each kind of disc.
- The lab test `storage/disc-burn` burns a CD-R of data, a CD of music
  and a DVD±RW in a real burner, and reads each back in the same drive.
  That is where real drives' timings, DVD+R and BD-R finalizing, and
  buffer underrun protection are checked; QEMU has no burner.
- A program with the write right to a drive can write its disc, as it
  can write a disk. It cannot load firmware into the drive or change its
  saved settings.
- MMC_COMMAND is a new request in the ABI, so every port is built again
  once.
