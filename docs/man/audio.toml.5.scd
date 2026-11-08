audio.toml(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

audio.toml - the output to play through, its volume and mute

# SYNOPSIS

_/system/state/audio/audio.toml_

# DESCRIPTION

The sound's settings, which *audiod*(8) gives the outputs of
_/devices/audio_ and *audio*(1) and the keyboard's volume keys change
(ADR 0320). The file is the system's state, as ALSA's saved mixer is on
Linux, rather than one of _/system/settings_, since audiod saves what
the volume keys do and no service may change the settings. Until the
first change there is no file, and the values are the defaults. They
are lines of TOML:
```
output = ""
volume = 70
muted = false
```

*output*
	The output to play through, by its name in _/devices/audio_, such
	as *virtio0*. Empty, or an output that is not there, for the one
	plugged in last, or else the first.

*volume*
	The volume of every output, 0 to 100: 70 by default. The kernel
	scales the samples by the cube of its fraction for an output with
	no volume of its own, so that each step sounds as loud a change as
	the last.

*muted*
	*true* to silence every output, keeping the volume for when it is
	*false* again.

A value that cannot be read is the default. Comments and other keys are
kept when audio or audiod change the file.

# SEE ALSO

*audio*(1), *audiod*(8)
