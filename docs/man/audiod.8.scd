audiod(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

audiod - keep the sound outputs as the settings say

# SYNOPSIS

*audiod*

# DESCRIPTION

audiod keeps the sound outputs of _/devices/audio_ as *audio.toml*(5)
says (ADR 0320). Spark runs it as the *audiod* service.

It gives every output the volume, or none while muted, and points
_/devices/audio/default_ at the output the settings name. With none
named, or the one named not there, it is the output plugged in last
while audiod ran (a USB headset), or else the first. Every two seconds
it looks again for outputs that came or went and for a changed file, and
does the same.

audiod reads the keyboard, _/devices/kbd_, beside the consoles: the
volume keys change the volume by 5 and unmute, and the mute key mutes or
unmutes. Each change is saved to the file and given to the outputs at
once, and logged:
```
audiod: volume 55%, muted
```

audiod plays nothing itself, and holds no output: programs play on the
outputs directly, one at a time (*audio*(1)).

# FILES

_/system/state/audio/audio.toml_
	The settings, which audiod writes when a key changes them; Spark
	makes the folder for it.

_/devices/audio_, _/devices/kbd_
	The outputs and the keyboard, which its service file grants.

# SEE ALSO

*audio*(1), *audio.toml*(5), *radix-files*(7), *service.toml*(5), *spark*(8)
