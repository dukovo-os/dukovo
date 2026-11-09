<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# The screen reader

The screen reader says what the consoles show, in any of eSpeak NG's
more than a hundred languages, through the default sound output
([Sound](sound.md)). It speaks over music or anything else playing, and
lets the output go a second after it stops. The full references are
reader(1), readerd(8), reader.toml(5), the *SCREEN READER* section of
consoled(8), espeak-ng(1) and ADR 0324.

## Turning it on

On the install stick, press S at the boot menu, after its tune, for
*Install Dukovo with speech* ([Installing Dukovo](installing.md)). On an
installed system, as root:

```
# reader on
on
# reader status
Screen reader: on
Voice:         en
Rate:          175 words a minute
Pitch:         50
readerd:       running (voice en rate 175 pitch 50 output hda0)
```

It then starts with the system each time, until `reader off`.

## What it says

- What programs write on the shown console, a line at a time. A long
  listing is cut short after 40 lines, with how many more there were.
- The characters typed, as the shell shows them, and what the cursor
  moves onto: the character for Left, Right, Backspace and Delete, the
  line for Up, Down, Home and End.
- "Console 2" and its cursor's line on Alt+F2, and so on.
- Notifications as they show.
- elevate's password prompt, apps' permission questions and the
  Ctrl+Alt+Delete menu, after two short rising tones. The tones come
  before the system's own prompts alone: a program that draws something
  like elevate's prompt is said without them. To be sure a sign-in is
  the system's, press Ctrl+Alt+Delete: its menu comes after the tones,
  and its Enter shows the sign-in. What you type at elevate's prompt is
  never said.

Any key stops what it was saying, and Ctrl alone stops it without doing
anything else.

## The reader's keys

Hold Caps Lock, or Insert, and press:

| Key | Says |
| --- | --- |
| Up, Down | the line above or below |
| Left, Right | the word before or after |
| Comma, Period | the character before or after |
| L | the line again |
| A | every line from there down |
| T | which console this is |
| Page Up, Page Down | faster, slower (kept for the next start) |
| H | these keys |

The review starts at the cursor each time a program writes. Press Caps
Lock twice quickly to turn capitals on or off as usual.

## Voice, rate and pitch

```
# reader voice de
# reader rate 220
# reader pitch 40
```

`espeak-ng --voices` lists the voices: a language such as `en-us` or
`fr`, with a variant after a plus sign, as `en+f3`. The rate is 80 to 450
words a minute.
