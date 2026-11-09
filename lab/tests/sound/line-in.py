# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""What the lab PC heard through its line in, for sound/tone.toml.

The machine's `audio test` plays 440 Hz on the left for 0.6 s, then
660 Hz on the right. For each channel this finds its tone, the longest
stretch well above the hiss, measures its pitch from its rising zero
crossings, and says how much quieter the other channel was meanwhile:

    left: 440 Hz (439.98 measured), the right 61 dB down
    left alone

A pitch within 1% of the tone's is given as the tone's; "alone" means
the other channel was at least 20 dB down. Only the standard library is
used, so any lab PC with Python 3 runs it.
"""

import math
import sys
import wave
from array import array

# The tones `audio test` plays (tone.rs in Dukovo's audio library).
TONES = {"left": 440.0, "right": 660.0}

# The recording is measured in blocks of this many milliseconds.
BLOCK_MS = 20


def read(path):
    """The rate and the left and right samples of a 16-bit stereo WAV file."""
    with wave.open(path, "rb") as recording:
        if recording.getsampwidth() != 2 or recording.getnchannels() != 2:
            sys.exit(f"{path} is not 16-bit stereo")
        rate = recording.getframerate()
        frames = recording.readframes(recording.getnframes())
    samples = array("h", frames)
    if sys.byteorder == "big":
        samples.byteswap()
    return rate, samples[0::2], samples[1::2]


def level(samples):
    """The root mean square of `samples`."""
    if not samples:
        return 0.0
    return math.sqrt(sum(s * s for s in samples) / len(samples))


def tone(samples, rate):
    """Where the tone in `samples` is, as (start, end), or None.

    The tone is the longest run of blocks at least a tenth as loud as the
    loudest, less a block at each end where it fades in or out; there is
    none if the loudest block is not ten times the hiss (the quietest
    fifth's level) or the run is under 100 ms.
    """
    size = rate * BLOCK_MS // 1000
    levels = [level(samples[i : i + size]) for i in range(0, len(samples) - size + 1, size)]
    if not levels:
        return None
    loudest = max(levels)
    hiss = sorted(levels)[len(levels) // 5]
    if loudest < 30 or loudest < 10 * hiss:
        return None
    best, start = (0, 0), None
    for i, each in enumerate(levels + [0.0]):
        if each >= loudest / 10:
            if start is None:
                start = i
        elif start is not None:
            if i - start > best[1] - best[0]:
                best = (start, i)
            start = None
    first, last = best[0] + 1, best[1] - 1
    if (last - first) * BLOCK_MS < 100:
        return None
    return first * size, last * size


def pitch(samples, rate):
    """The frequency of the sine in `samples`, from its rising zero
    crossings, each placed between two samples by interpolation. A
    crossing counts only once the signal has been a quarter of its level
    below zero, so hiss near zero adds none."""
    mean = sum(samples) / len(samples)
    below = -level([s - mean for s in samples]) / 4
    crossings, armed = [], False
    previous = samples[0] - mean
    for i in range(1, len(samples)):
        now = samples[i] - mean
        if now < below:
            armed = True
        elif armed and previous < 0 <= now:
            crossings.append(i - 1 + -previous / (now - previous))
            armed = False
        previous = now
    if len(crossings) < 3:
        return None
    return (len(crossings) - 1) * rate / (crossings[-1] - crossings[0])


def main():
    rate, left, right = read(sys.argv[1])
    channels = {"left": (left, right, "right"), "right": (right, left, "left")}
    heard = True
    for name, (samples, others, other) in channels.items():
        found = tone(samples, rate)
        if found is None:
            print(f"{name}: no tone")
            heard = False
            continue
        start, end = found
        frequency = pitch(samples[start:end], rate)
        if frequency is None:
            print(f"{name}: no pitch")
            heard = False
            continue
        expected = TONES[name]
        said = expected if abs(frequency - expected) <= expected / 100 else frequency
        down = 20 * math.log10(level(samples[start:end]) / max(level(others[start:end]), 1.0))
        print(f"{name}: {said:.0f} Hz ({frequency:.2f} measured), the {other} {down:.0f} dB down")
        if down >= 20:
            print(f"{name} alone")
    sys.exit(0 if heard else 1)


if __name__ == "__main__":
    main()
