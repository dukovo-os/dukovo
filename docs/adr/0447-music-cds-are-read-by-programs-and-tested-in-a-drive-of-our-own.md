<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0447. Music CDs are read by programs, and tested in a drive of our own

- Status: Accepted
- Date: 2026-11-10

## Context

ADR 0431 finds a music CD in a drive and mounts nothing of it, as its
tracks are no filesystem. Roadmap 11h's audio CD step is to play them
and rip them.

- A music CD is read with MMC's READ CD, sector type CD-DA: 2,352 bytes a
  sector, 588 frames of two 16-bit samples at 44,100 frames a second,
  with no error correction past the drive's own. Its table of contents
  comes from READ TOC/PMA/ATIP, format 0 for the tracks and 1 for the
  sessions, and its CD-Text from format 5: packs of 18 bytes from the
  lead-in, each with a CRC, holding the disc's and each track's title and
  performer.
- Linux has CDROMREADTOCHDR, CDROMREADTOCENTRY and CDROMREADAUDIO, and
  leaves CD-Text to commands passed through raw with SG_IO; rippers
  such as cdparanoia read with them.
- A track's pregap, the silence before index 1, is counted in the track
  before it by the table of contents, which gives only where each track's
  index 1 starts. On an Enhanced CD (CD-Extra) the audio is in the first
  session and a data track starts the second: the last audio track ends
  11,400 sectors before it, where the first session's lead-out and the
  second's lead-in and pregap lie.
- QEMU 8.2's CD drives (`ide-cd`, `scsi-cd`, `usb-storage` with a CD)
  take an image of 2,048-byte sectors and answer READ CD only for data.
  None holds a music CD, or CD-Text, so a boot test needs a drive QEMU
  does not have. QEMU's `usb-redir` device plugs a USB device in that
  another program serves over a socket, in usbredir's protocol.
- Lossless rips are FLAC (RFC 9639) or WAV. Ogg Opus and Vorbis need
  libopus and libvorbis, in C: no encoder of either in Rust has a
  permissive license.

## Decision

- **radix-block(2) has READ_TOC and READ_AUDIO,** Dukovo's own requests,
  as FORMAT_TRACK is. READ_TOC gives a READ TOC answer as MMC lays it
  out, in format 0, 1, 2 or 5, with block numbers for addresses.
  READ_AUDIO reads 1 to 27 sectors of audio from a block number into a
  buffer, as CDROMREADAUDIO does. Both need the read right, and work on a
  drive whose disc is no filesystem, a device of no sectors.
- **A drive's commands go through its thread.** Each request is handed
  to the drive's driver thread (`removable::Drive::command`), as EJECT
  is, so that it never races the drive's polling. The thread runs it on
  the drive's transport, AHCI, IDE or USB, through 16 pages of DMA
  memory it keeps (`optical::serve_asked`), and answers it by number: an
  answer that comes after its caller gave up is not taken for the next
  request's. A UNIT ATTENTION is retried once. No disc is `no_device`, a
  command the drive refuses is `invalid_argument`, and anything else is
  `io`.
- **audiocd(1) lists, plays and rips** (systools). It works the drive
  named, or the first that holds a music CD as `/kernel/block/<drive>/disc`
  says, or else a CD of data and audio. Tracks come from format 0, with
  an Enhanced CD's gap from format 1. Titles and performers come from
  CD-Text's first block in ISO 8859-1, packs whose CRC does not hold
  left out; MS-JIS text is not read (`audio::cd`). `play` plays through
  an output of `/devices/audio` as audio(1) plays a file. `rip` writes a
  file a track, named by number and title, as FLAC tagged from the
  CD-Text (TITLE, ARTIST, ALBUM, ALBUMARTIST, TRACKNUMBER, TRACKTOTAL),
  or as WAV with `--wav`. A file is written as `.part` until it is whole.
  A sector that does not read is read again twice. There is no jitter
  correction, as drives since the late 1990s read audio where it is
  (accurate stream).
- **FLAC is the audio crate's own encoder (`audio::flac`),** written from
  RFC 9639:
  - blocks of 4,096 frames, each channel a constant (silence), a fixed
    predictor of order 0 to 4 with its residual Rice-coded in
    partitions, or verbatim;
  - a pair as the cheapest of left and right, left and side, side and
    right, or mid and side;
  - the samples' MD5 in the STREAMINFO.

  A file is a few per cent larger than libFLAC's at its default level,
  which adds linear prediction; that can come later without changing the
  format. Its decoder reads back what it writes, checking every CRC and
  the MD5, for the tests. The MD5 is RustCrypto's `md-5`, which the
  accounts crate has too.
- **Ogg Opus and Vorbis wait for the media service's libopus port.**
- **The tests have a USB CD drive of their own, `ob disc-drive`**
  (`tools/xtask/src/disc_drive`).
  - It is a USB mass storage device (Bulk-Only) of SCSI MMC commands:
    INQUIRY, TEST UNIT READY, REQUEST SENSE, READ CAPACITY, READ(10), READ
    TOC formats 0, 1, 2 and 5, READ CD, GET CONFIGURATION, GET EVENT
    STATUS NOTIFICATION, MODE SENSE's page 2A, START STOP UNIT, PREVENT
    ALLOW MEDIUM REMOVAL, READ DISC INFORMATION and SET CD SPEED.
  - It holds a disc made from a cue sheet: BINARY, MOTOROLA and WAVE
    files; AUDIO, MODE1/2048 and MODE1/2352 tracks; INDEX 00 and 01,
    PREGAP, and TITLE and PERFORMER as CD-Text. It can also hold an ISO.
  - It speaks usbredir's protocol, version 0.7, to QEMU's `usb-redir`
    over a socket, as a USB device redirected from another machine is.
    It is written from the protocol's description; no code is taken.
  - A boot test plugs it in with `-chardev socket` and `usb-redir`, and
    puts a disc in, takes it out, or presses its button as the test
    goes. `ob disc-drive <socket> [<disc>]` runs it alone, for the lab
    and by hand.

## Consequences

- The boot test `audio-cd` plugs the emulated drive in with a disc of
  two tracks with CD-Text, a pregap and seven seconds of tones, silence
  and noise:
  - The kernel says a music CD went in and mounts nothing.
  - `audiocd` lists the tracks by title.
  - It rips both to FLAC and one to WAV. Each file's SHA-256 matches the
    file the host makes of the disc's samples, and each FLAC file decodes
    to its track's samples on the host.
  - It plays track 1 through a virtio sound card, and QEMU's recording
    holds 440 Hz on the left and 660 Hz on the right for four seconds.
  - Ejected, the drive is empty and `audiocd` finds no music CD; put
    back, the disc is found again.
- Unit tests cover the FLAC encoder and decoder (round trips, a file
  written a piece at a time, silence, mono, other rates, damage
  refused), the table of contents (pregaps, an Enhanced CD's gap) and
  CD-Text (tabs, ISO 8859-1, a bad CRC, MS-JIS), and the emulated
  drive's cue sheets, CD-Text, MMC commands and protocols. ffmpeg's FLAC
  decoder reads the encoder's files back bit for bit.
- The lab test `storage/audio-cd` rips and plays a real music CD in a
  real drive. That is where jitter, a scratched disc, and drives' own
  CD-Text answers are checked.
- READ_TOC and READ_AUDIO are new requests in the ABI, so every port is
  built again once.
