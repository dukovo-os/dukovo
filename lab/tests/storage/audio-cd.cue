REM SPDX-FileCopyrightText: 2026 The Dukovo contributors
REM SPDX-License-Identifier: Apache-2.0 OR MIT
REM The lab's music CD (lab/tests/storage/audio-cd.toml, ADR 0447): track 1
REM is 441 Hz on the left and 630 Hz on the right for four seconds; track 2
REM a second of silence, then 882 Hz on both sides, the left louder, for
REM four. Each tone repeats whole every 700 frames, so the file packs small.
TITLE "Lab Sessions"
PERFORMER "Dukovo Lab"
FILE "audio-cd.bin" BINARY
  TRACK 01 AUDIO
    TITLE "First Light"
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    TITLE "Hum"
    INDEX 00 00:04:00
    INDEX 01 00:05:00
