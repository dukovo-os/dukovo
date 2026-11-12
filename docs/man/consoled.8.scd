consoled(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

consoled - the virtual consoles on the screen and keyboard

# SYNOPSIS

*consoled* [*--consoles* _n_] [*--shell* _path_]

# DESCRIPTION

consoled draws Dukovo's virtual consoles, *tty1* to *tty6*, on the
screen, one at a time, and types what is pressed on the keyboard into
the one shown. Each console is a terminal for *TERM=linux*, a VT100 with
the Linux console's colours, keys and escape sequences, running the
sign-in, *login*(1), on a pseudo-terminal of its own (*radix-posix*(7)).

The Radix kernel shows its log on the screen while the machine starts.
When consoled takes the screen (_/devices/fb0_, *radix-framebuffer*(2)) the
kernel stops drawing on it, and when consoled ends the screen is the
log's again; *dmesg*(1) shows the kernel's messages meanwhile. The keys
come from _/devices/kbd_ (*radix-input*(2)): the PS/2 keyboard's events
and every USB keyboard's, which can be plugged in and pulled out at any
time.

Spark starts consoled at boot as the *consoled* service
(_/system/settings/spark/services/consoled.toml_, *spark*(8)), first and
stopped last, and starts it again if it fails. Its file asks for
*admin.hardware*, so that root's shells may set the fans, the LEDs and
the ARGB header with *ectool*(8), and for the whole network with
*admin.network*, so that they may connect, listen on any port and set
the interfaces up with *ip*(8). Without a screen, as on
a machine with only a serial console, consoled logs *no screen for
consoles* and exits with 0, and stays stopped.

# SESSIONS

tty1's sign-in starts with consoled; each other console's starts the
first time it is shown. consoled runs _/system/commands/login_ as root
on the console's terminal, in a session of its own, with *TERM=linux*
and the screen's size on the terminal. Before it, the console shows
_/system/settings/issue_, in which *\\s* is the system's name, *\\r* its
release, *\\l* the console's name, *\\n* the host's name, *\\v* the
kernel's version and *\\m* the machine:

```
Dukovo Server 0.15.0 (tty1)

Name:
```

login asks for a name and a password, and *authd*(8) starts the
person's session on that terminal, acting for them; on a machine with
no account yet it first makes the owner's (*login*(1), ADR 0250). Each
console signs in on its own, so two people, or one person twice, can be
signed in on two consoles at once.

Above the sign-in prompt, in issue's place, each console shows the
welcome banner: Duke and the name in plain ASCII, the tagline, and the
system's name and version (ADR 0261). *welcome.toml*(5) says when it
shows and what is in it; a screen too small for Duke gets less. Consoles
that start root's shell with no sign-in show issue's text. Under it, until
the next start, each shows what the health check said of a failed start,
_/system/live/sysupdate/failure_ (*sysupdate*(8), ADR 0501).

When the sign-in ends, because the session it started ended, as with
*exit*, or because its input did, the console clears and the sign-in
starts again. One that ends within 2 seconds five times in a row waits
10 seconds before the next.

With *--shell*, or with *console-sign-in=off* on the kernel's command
line (*dukovo-boot*(7)), there is no sign-in: each console runs root's
login shell from _/system/settings/passwd_ (*nukesh*(1) by default), as
*-nukesh*, with *HOME*, *SHELL*, *USER*, *LOGNAME* and *TERM=linux* set,
and with no password asked, and consoled logs *console-sign-in=off:
root's shell on each console, no sign-in*. Only someone who can already
change the kernel's command line, at the boot menu or in the boot
volume, can turn it off; the boot tests do, to type at root's shell.

With *install* on the kernel's command line, as the install stick's
*Install Dukovo* entry starts the live system, tty1's first session runs
*dukovo-install*(8)'s screens as root in the shell's place, with no
issue text and no cursor until they draw, and consoled logs *install:
the installer's screens on tty1*. The font is then the largest that
gives the screens' 100 columns by 30 rows, where the consoles' own
wants 40 rows: 12x24 on a 1280x800 screen, 16x32 on 1920x1080. When the
screens end, by Live console on their first page or at the end of the
install, tty1 clears and runs the shell as the other consoles do (ADRs
0310 and 0402). With *console=ttyS0* as well, as the stick's *Install
Dukovo (serial console)* entry gives it, and a first serial port to put
them on, the screens are *serial-console*(8)'s there instead: tty1 runs
the shell from the start, and consoled logs *install: the installer's
screens are on the serial port, ttyS0* (ADR 0318).

With *rescue=*_uuid_ on the kernel's command line, as the Rescue entry of
an installed system's boot menu starts the live system, every console
runs *dukovo-rescue*(8)'s screens as root, in place of the sign-in or the
shell, and again whenever they end, and consoled logs *rescue: Rescue's
screens on every console* (ADR 0460). A shell is one of Rescue's
choices, after an administrator's password.

# NOTIFICATIONS

consoled shows the notifications *notifyd*(8) passes on, such as
*notify-send*(1)'s, on the bottom line of the screen, over the shown
console's last line: the name of the program that sent it, its title
and its body on one line, cut with *...* if it is too long, in white on
blue, or in red for a critical one and grey for a low one (ADR 0212). It
stays for its time, 8 seconds unless it asked for another; a critical
one, or one of 0 seconds, stays until a key is pressed, and the key
still goes to the console. A newer notification takes the line at once,
and *notifications*(1) lists them all. consoled logs *showing
notifications from notifyd* when it finds notifyd, and tries again every
5 seconds while it is not running.

# PASSWORDS

When *elevate*(8), or *sudo*(8), needs its person's password in a pane
on one of the consoles, consoled asks for it, not elevate (ADR 0251).
*authd*(8) tells consoled, which shows the prompt on that console's
bottom line, in black on cyan:

```
 [elevate] Password for alice, to run /system/commands/user add bob:
```

The keys typed while it shows are consoled's: what is typed never
shows and never reaches the console's programs, *Enter* sends it to
authd alone, on a connection of its own, *Backspace* takes back a
character and *Ctrl+U* all of them, and *Esc* or *Ctrl+C* close the
prompt, and elevate says no password was typed. Alt+F1 to Alt+F6 still
show the other consoles; the prompt waits on its own. No program can
read the password, and none can draw this prompt so that it takes
keys: *Ctrl+Alt+Delete*, which no program receives, brings the real one
back cleared, or shows consoled's menu if there is none. A wrong
password is asked again by a new prompt, after authd's wait, and
elevate says *Sorry, try again.* on the terminal.

*bundled*(8)'s questions about apps' permissions come the same way
(ADR 0224), on the bottom line, in black on cyan, or white on red for
one with a warning:

```
 [permission] Notes (org.example.notes) asks to: use your documents folder, to read and change it      O: once    A: always    D: don't allow
```

*O* allows it this time, *A* from now on, *D* never, and *Esc* or
*Ctrl+C* close it; *bundle grant*'s question takes *Y* or *N*. No other
key does anything while it shows, and none reaches the console's
programs. On a console narrower than the question, its words are cut
with *...*, never its keys.

consoled logs *asking for passwords on the consoles for authd* when it
finds authd, and tries again every 5 seconds while it is not running;
meanwhile, and on any other terminal, as over SSH, elevate asks itself.
With root's shell on each console and no sign-in, consoled asks for no
passwords.

# SCREEN READER

While *readerd*(8) runs, consoled tells it what to say (ADR 0324):

- what programs write to the shown console, each line as it ends and
  the rest once the console has been quiet for 80 ms, so that a typed
  character is said as the shell echoes it; past 40 lines at once, the
  rest are summed up as *and 12 more lines*;
- what a key moved the cursor onto when the program wrote nothing in
  answer: the character for *Left*, *Right*, *Backspace* and *Delete*,
  the line for *Up*, *Down*, *Home* and *End*;
- *Console 2* and the cursor's line when another console is shown, and
  the whole screen when readerd starts listening;
- notifications, as they show;
- consoled's own prompts, menu and notes, all their words and the keys
  that answer them, after the trusted sound: two rising tones that only
  consoled's words are said after. No program can have them said after
  it, so a program that draws a prompt like consoled's is never mistaken
  for it. What is typed at a password prompt is never said.

Any key pressed for a program stops what was being said, and *Ctrl*
alone stops it too. The reader key, *Caps Lock* or *Insert*, makes the
keys pressed while it is held the reader's, which no program receives:

*Up*, *Down*
	The line above or below the review cursor, which starts at the
	cursor each time a program writes.

*Left*, *Right*
	The word before or after it.

*Comma*, *Period*
	The character before or after it.

*L*, *A*
	The line again, or every line from it down.

*T*
	Which console is shown.

*Page Up*, *Page Down*
	Faster and slower, by 25 words a minute, which readerd keeps.

*H*
	The reader's keys.

Pressed twice within 0.4 seconds, Caps Lock or Insert is itself, for the
program. consoled logs *the screen reader is listening* when it finds
readerd, and tries again every 2 seconds while it is not running; until
then the keys are the programs' as always.

A program that draws a whole screen, as the installer does, can name
what has its focus better than its screen's changes would (ADR 0325):
*ESC ] 7700 ;* _text_ *ST* (or ended by *BEL*) has the reader say _text_
at once, over what it was saying. From then on, what the program draws
is not read by its changes, until it leaves the alternate screen
(*ESC [ ? 1049 l*), as every full-screen program does as it ends, or
the terminal is reset (*ESC c*). Any program may; its words never come after
the trusted sound. While readerd is not listening, the last _text_ is
kept: when readerd starts, or the console is shown again, the reader
says the console's number and that text, in place of the screen.
Other terminals ignore the sequence.

# TEXT SIZE AND KEYS

For those who need bigger text, or cannot hold two keys at once or
press one briefly (ADR 0325), as _accessibility.toml_ sets
(*accessibility.toml*(5)):

*Ctrl+Alt+Plus*, *Ctrl+Alt+Minus*
	Make the text a step bigger or smaller: *normal*, the font the
	screen's size chooses, then *larger* and *largest*, the next of the
	12x24 and 16x32 fonts and then those drawn twice their size, as far
	as the screen keeps at least 40 by 12 cells. Each console gets as
	many cells as then fit, and its program is told, as when a terminal
	window is resized (SIGWINCH). The size is kept. *Plus* and *Minus*
	are the keys that type them in the layout, or the keypad's; in US
	English, *Ctrl+Alt+=* is Ctrl+Alt+Plus.

*Shift*, five times
	Turns sticky keys on or off, and says so on the bottom line. With
	sticky keys, *Shift*, *Ctrl* and *Alt* pressed and let go alone hold
	for the next key; pressed twice, they hold until pressed again, and
	a third time lets go. *Ctrl*, *Alt*, then *Delete* is
	Ctrl+Alt+Delete. A modifier held down with a key is as always.

With slow keys, a key counts only once it has been held for the delay,
300 ms unless the file says otherwise: a key brushed on the way to
another types nothing, and one held on repeats as usual. Every key
waits, Shift and the others too.

The screen reader says each change: *Text larger*, *shift*, *shift
locked*, *Sticky keys on*.

# BRAILLE DISPLAYS

consoled shows a braille display the consoles through *brltty*(1),
which *brailled*(8) starts when one is plugged in (ADR 0326). It listens
on _/system/live/consoled/braille_, which only root can open, for one
display at a time, and each time what the shown console shows changes
it sends the screen whole, a line of its number, size and cursor:
```
screen NUMBER COLUMNS ROWS X Y
```
then each row as text. The rows are the console's as the screen shows
them, the scrollback while it is moved back, and one more at the
bottom: consoled's own line, with its prompts, its menu, its notes and
the notifications, or blank. No program writes that row. While consoled
asks for something there, the cursor is on it, after a prompt's words
or at the start of the menu. Only the newest screen waits for a display
that reads slowly.

The display types back through consoled, a line for each key:
*key* _name_, then *shift*, *ctrl* and *alt* as held, where _name_ is a
character as *U+0041*, or a key: *enter*, *tab*, *backspace*, *escape*,
*insert*, *delete*, *home*, *end*, *pageup*, *pagedown*, *up*, *down*,
*left*, *right* or *f1* to *f12*; and *switch* _n_ shows console _n_.
consoled takes each as from its keyboard, so its prompts and menu take
them first: a person answers elevate's password and reaches the menu
from the display. A key no keyboard has, or a control character, is
refused, and a line that is not one of these is dropped.

# KEYS

*Alt+F1* to *Alt+F6*
	Show that console. *Alt+Left* and *Alt+Right* go to the program on
	the console, as in a terminal window: *nukesh*(1) goes back and
	forward through its folders with them.

*Shift+Page Up*, *Shift+Page Down*
	Look back through the console's last 1000 lines and forward again.
	Any key typed goes back to the end.

*Scroll Lock*
	Hold the shown console: what its programs write waits until Scroll
	Lock is pressed again.

*Ctrl+Alt+Delete*
	The secure attention key, which no program ever receives (ADR
	0251). When the shown console asks for a password, it clears what
	was typed there, so that what is typed next goes to consoled alone.
	Otherwise it shows consoled's menu on the bottom line: *Enter*
	shows the sign-in, on the shown console if nobody is signed in
	there, or else on the first console where nobody is; *R* runs
	*reboot*(8) and *P* *poweroff*(8); *Esc* closes it. With root's
	shell on each console and no sign-in, the menu has no *Enter*.

*Caps Lock*, *Num Lock*
	As on any keyboard; every keyboard's lights follow them. The keypad
	types digits while Num Lock is on, which it is from the start. While
	the screen reader runs, Caps Lock is its key, and pressed twice is
	Caps Lock (*SCREEN READER*).

The keys type in the layout _/system/settings/keyboard_ names, US
English unless it names another (*keymap*(1)), which every keyboard
shares: consoled reads it when it starts and when it is sent SIGHUP,
which *service consoled reload* does, as it does *accessibility.toml*(5). In a layout that has them, Right
Alt is AltGr, and a dead key puts its accent on the letter typed after
it (*keymap*(5)). Ctrl with a key whose character has no control
character sends the control character of the key's US one. The other
keys send what the Linux console sends: *F1* to *F5* as *ESC [ [ A* to *ESC [ [ E*, *F6* to *F12*
as *ESC [ 17 ~* to *ESC [ 24 ~*, *Home* to *Page Down* as *ESC [ 1 ~* to
*ESC [ 6 ~*, the arrows as *ESC [ A* to *ESC [ D* (*ESC O A* in
application cursor mode); *Alt* with a key sends *ESC* before it.

# TERMINAL

The consoles understand what the Linux console's terminfo entry
(*linux*) lists: cursor movement, erasing, inserting and deleting
characters and lines, scrolling regions, tab stops, the alternate
character set for line drawing, saving and restoring the cursor,
insert, origin, autowrap and reverse-video modes, application cursor
and keypad modes, and SGR's bold, dim, underline, reverse, the 16
colours, the 256 colours and true colour, the latter two shown as the
nearest of the 256. Wide characters take two cells. The parser is the
vte crate's, from the Alacritty project.

A program can change the colour each of the 256 shows, on its own
console, as xterm and the Linux console let it: *ESC ] 4 ;* _n_ *;*
_colour_ *ST* sets index _n_ to _colour_, *rgb:*_rr_*/*_gg_*/*_bb_ (one
to four hex digits each) or *#*_rrggbb_, or with *?* asks for it; *ESC ]
104 ST* gives every index its own colour back, or with indices, those;
the Linux console's *ESC ] P* _nrrggbb_ and *ESC ] R* do the same for
the first sixteen, and end at the next escape sequence. The screen's
margins round the cells are index 0's colour, the background. A new
session, and *ESC c*, start with the colours as they were. The
installer's screens set their design's sixteen while they show.

# OPTIONS

*--consoles* _n_
	Have _n_ consoles, 1 to 12, rather than 6.

*--shell* _path_
	Start _path_ as root's login shell in each session, with no
	sign-in.

*--help*, *--version*
	Show how to call it, or its version.

# FILES

_/devices/fb0_
	The screen.

_/devices/kbd_
	The keyboards' events.

_/system/settings/issue_
	What each console shows before its session. Its default is in
	_/system/resources/default-settings/issue_.

_/system/settings/console/welcome.toml_
	The welcome banner (*welcome.toml*(5)).

_/system/settings/console/accessibility.toml_
	The text size, sticky keys and slow keys
	(*accessibility.toml*(5)), which consoled writes as they are changed
	from the keyboard.

_/system/settings/keyboard_
	The keyboard layout's setting (*keymap*(1)).

_/system/settings/keymaps/name.keymap_
	One's own layouts (*keymap*(5)).

_/system/settings/spark/services/consoled.toml_
	The service.

_/system/live/notify/socket_
	notifyd's socket, which consoled follows.

_/system/live/authd/socket_
	authd's socket, which consoled asks for prompts and answers them on.

_/system/live/readerd/socket_
	readerd's socket, which consoled tells what to say.

_/system/live/consoled/braille_
	The socket a braille display's BRLTTY reads the consoles on.

# SEE ALSO

*login*(1), *authd*(8), *elevate*(8), *spark*(8), *reboot*(8),
*keymap*(1), *keymap*(5), *welcome.toml*(5), *accessibility.toml*(5),
*nukesh*(1), *dmesg*(1),
*notifyd*(8), *notify-send*(1), *readerd*(8), *reader*(1), *brailled*(8),
*braille*(1), *brltty*(1), *dukovo-boot*(7),
*radix-framebuffer*(2), *radix-input*(2), *radix-files*(7),
*serial-console*(8)
