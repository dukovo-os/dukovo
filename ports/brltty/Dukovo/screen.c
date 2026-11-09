/*
 * SPDX-FileCopyrightText: 2026 The Dukovo contributors
 * SPDX-License-Identifier: Apache-2.0 OR MIT
 *
 * BRLTTY's screen driver for Dukovo (ADR 0326): the consoles, as
 * consoled(8) shows them.
 *
 * consoled listens on a socket only root may open,
 * /system/live/consoled/braille. Each time what the shown console shows
 * changes, consoled writes it whole:
 *
 *     screen NUMBER COLUMNS ROWS X Y
 *
 * then ROWS lines of COLUMNS characters each, in UTF-8. The last row is
 * consoled's own: its prompts, its Ctrl+Alt+Delete menu and its notes,
 * or blank. No program can write there, so what a person reads on it is
 * consoled's, as the trusted sound marks it for the screen reader. While
 * consoled asks for something there, X and Y are where its answer is
 * typed.
 *
 * What the display's keys type, the driver writes back a line at a time:
 *
 *     key NAME [shift] [ctrl] [alt]
 *     switch NUMBER
 *
 * NAME is a character as U+XXXX, or enter, tab, backspace, escape, left,
 * right, up, down, pageup, pagedown, home, end, insert, delete, or f1 to
 * f24. consoled takes it as if typed on the keyboard, so it reaches its
 * prompts as the keyboard's would. switch shows another console.
 *
 * The driver reads the socket whenever it has something, keeps the last
 * screen whole, and tells BRLTTY the screen changed. If consoled is not
 * listening yet, or goes away, the display says so, and the driver tries
 * again each time BRLTTY looks.
 */

#include "prologue.h"

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include "log.h"
#include "utf8.h"
#include "async_handle.h"
#include "async_io.h"

#include "scr_driver.h"

/* Where consoled listens. */
#define SOCKET_PATH "/system/live/consoled/braille"

/* The most a screen may be: consoled's screens are far smaller. */
#define MAXIMUM_COLUMNS 1000
#define MAXIMUM_ROWS 500

/* The connection to consoled, while there is one. */
static int consoledSocket = -1;
static AsyncHandle consoledMonitor = NULL;

/* What has come from consoled and is not yet a whole screen. */
static char *pending = NULL;
static size_t pendingLength = 0;
static size_t pendingSize = 0;

/* The last whole screen. */
static int screenNumber = 0;
static int screenColumns = 0;
static int screenRows = 0;
static int cursorColumn = 0;
static int cursorRow = 0;
static wchar_t *screenText = NULL;

/* Why there is no screen to show, while there is none. */
static const char *problem = "consoled not listening";

static void
forgetScreen (void) {
  free(screenText);
  screenText = NULL;
  screenColumns = 0;
  screenRows = 0;
}

static void
disconnect (const char *why) {
  if (consoledMonitor) {
    asyncCancelRequest(consoledMonitor);
    consoledMonitor = NULL;
  }

  if (consoledSocket != -1) {
    close(consoledSocket);
    consoledSocket = -1;
    logMessage(LOG_CATEGORY(SCREEN_DRIVER), "consoled: %s", why);
  }

  pendingLength = 0;
  forgetScreen();
  problem = "consoled not listening";
}

/*
 * The length of the line at the start of `text`, with its line feed, or
 * 0 if it has not all come.
 */
static size_t
lineLength (const char *text, size_t length) {
  const char *end = memchr(text, '\n', length);
  return end? (size_t)(end - text) + 1: 0;
}

/*
 * Takes one screen from the start of what has come, if it has all come:
 * the number of bytes it took, 0 if more is needed, or -1 if what came is
 * not a screen.
 */
static ssize_t
takeScreen (void) {
  size_t header = lineLength(pending, pendingLength);
  if (!header) return 0;

  int number, columns, rows, x, y;
  {
    char line[100];
    if (header >= sizeof(line)) return -1;
    memcpy(line, pending, header);
    line[header] = 0;

    if (sscanf(line, "screen %d %d %d %d %d", &number, &columns, &rows, &x, &y) != 5) return -1;
  }

  if ((columns < 1) || (columns > MAXIMUM_COLUMNS)) return -1;
  if ((rows < 1) || (rows > MAXIMUM_ROWS)) return -1;

  /* Every row must have come before the screen is taken. */
  size_t offset = header;
  for (int row=0; row<rows; row+=1) {
    size_t length = lineLength(pending + offset, pendingLength - offset);
    if (!length) return 0;
    offset += length;
  }

  wchar_t *text = malloc(sizeof(*text) * columns * rows);
  if (!text) {
    logMallocError();
    return -1;
  }

  const char *line = pending + header;
  for (int row=0; row<rows; row+=1) {
    size_t length = lineLength(line, pendingLength - (line - pending)) - 1;
    const char *next = line + length + 1;
    wchar_t *cell = text + (row * columns);
    int column = 0;

    /* A row with fewer characters than columns is filled with blanks. */
    while ((length > 0) && (column < columns)) {
      wint_t character = convertUtf8ToWchar(&line, &length);
      if (character == WEOF) break;
      cell[column++] = character;
    }

    while (column < columns) cell[column++] = WC_C(' ');
    line = next;
  }

  free(screenText);
  screenText = text;
  screenNumber = number;
  screenColumns = columns;
  screenRows = rows;
  cursorColumn = (x >= 0 && x < columns)? x: 0;
  cursorRow = (y >= 0 && y < rows)? y: 0;
  return offset;
}

ASYNC_MONITOR_CALLBACK(dkConsoledMonitor) {
  if (pendingSize - pendingLength < 0X1000) {
    size_t size = pendingSize? pendingSize * 2: 0X10000;
    char *bigger = realloc(pending, size);

    if (!bigger) {
      logMallocError();
      disconnect("out of memory");
      return 0;
    }

    pending = bigger;
    pendingSize = size;
  }

  ssize_t count = read(consoledSocket, pending + pendingLength, pendingSize - pendingLength);
  if (count < 0) {
    if ((errno == EAGAIN) || (errno == EINTR)) return 1;
    disconnect(strerror(errno));
    mainScreenUpdated();
    return 0;
  }

  if (count == 0) {
    /* The monitor is cancelled with the socket: this callback ends it. */
    consoledMonitor = NULL;
    disconnect("went away");
    mainScreenUpdated();
    return 0;
  }

  pendingLength += count;
  int updated = 0;

  while (1) {
    ssize_t taken = takeScreen();
    if (taken == 0) break;

    if (taken < 0) {
      consoledMonitor = NULL;
      disconnect("sent what is not a screen");
      mainScreenUpdated();
      return 0;
    }

    memmove(pending, pending + taken, pendingLength - taken);
    pendingLength -= taken;
    updated = 1;
  }

  if (updated) mainScreenUpdated();
  return 1;
}

/* Connects to consoled, if it listens: true once connected. */
static int
connectToConsoled (void) {
  if (consoledSocket != -1) return 1;

  int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
  if (fd == -1) {
    logSystemError("socket");
    return 0;
  }

  struct sockaddr_un address;
  memset(&address, 0, sizeof(address));
  address.sun_family = AF_UNIX;
  strncpy(address.sun_path, SOCKET_PATH, sizeof(address.sun_path) - 1);

  if (connect(fd, (struct sockaddr *)&address, sizeof(address)) == -1) {
    close(fd);
    return 0;
  }

  consoledSocket = fd;
  pendingLength = 0;

  if (!asyncMonitorFileInput(&consoledMonitor, consoledSocket, dkConsoledMonitor, NULL)) {
    disconnect("cannot be read");
    return 0;
  }

  logMessage(LOG_CATEGORY(SCREEN_DRIVER), "consoled: connected");
  problem = "waiting for consoled";
  return 1;
}

/* Writes `line` to consoled whole: true if it went. */
static int
sendLine (const char *line) {
  if (!connectToConsoled()) return 0;

  size_t length = strlen(line);
  while (length > 0) {
    ssize_t count = write(consoledSocket, line, length);

    if (count < 0) {
      if (errno == EINTR) continue;
      logSystemError("write to consoled");
      return 0;
    }

    line += count;
    length -= count;
  }

  return 1;
}

static int
construct_DukovoScreen (void) {
  connectToConsoled();
  return 1;
}

static void
destruct_DukovoScreen (void) {
  disconnect("closed");
  free(pending);
  pending = NULL;
  pendingSize = 0;
}

/* BRLTTY looks again on its own only while consoled is not connected. */
static int
poll_DukovoScreen (void) {
  return consoledSocket == -1;
}

static int
refresh_DukovoScreen (void) {
  connectToConsoled();
  return 1;
}

static int
currentVirtualTerminal_DukovoScreen (void) {
  return screenNumber;
}

static void
describe_DukovoScreen (ScreenDescription *description) {
  if (!screenText) {
    description->unreadable = problem;
    description->cols = strlen(problem);
    description->rows = 1;
    description->posx = 0;
    description->posy = 0;
    return;
  }

  description->number = screenNumber;
  description->cols = screenColumns;
  description->rows = screenRows;
  description->posx = cursorColumn;
  description->posy = cursorRow;
}

static int
readCharacters_DukovoScreen (const ScreenBox *box, ScreenCharacter *buffer) {
  if (!screenText) {
    setScreenMessage(box, buffer, problem);
    return 1;
  }

  if (!validateScreenBox(box, screenColumns, screenRows)) return 0;

  ScreenCharacter *target = buffer;
  for (int row=0; row<box->height; row+=1) {
    const wchar_t *source = screenText + ((box->top + row) * screenColumns) + box->left;

    for (int column=0; column<box->width; column+=1) {
      target->text = *source++;
      target->color.vgaAttributes = VGA_COLOR_DEFAULT;
      target += 1;
    }
  }

  return 1;
}

/* The name consoled knows a special key by, or NULL. */
static const char *
specialKeyName (ScreenKey key) {
  static char function[4];

  switch (key) {
    case SCR_KEY_ENTER:        return "enter";
    case SCR_KEY_TAB:          return "tab";
    case SCR_KEY_BACKSPACE:    return "backspace";
    case SCR_KEY_ESCAPE:       return "escape";
    case SCR_KEY_CURSOR_LEFT:  return "left";
    case SCR_KEY_CURSOR_RIGHT: return "right";
    case SCR_KEY_CURSOR_UP:    return "up";
    case SCR_KEY_CURSOR_DOWN:  return "down";
    case SCR_KEY_PAGE_UP:      return "pageup";
    case SCR_KEY_PAGE_DOWN:    return "pagedown";
    case SCR_KEY_HOME:         return "home";
    case SCR_KEY_END:          return "end";
    case SCR_KEY_INSERT:       return "insert";
    case SCR_KEY_DELETE:       return "delete";

    default:
      if ((key >= SCR_KEY_F1) && (key <= SCR_KEY_F24)) {
        snprintf(function, sizeof(function), "f%d", (int)(key - SCR_KEY_F1 + 1));
        return function;
      }

      return NULL;
  }
}

static int
insertKey_DukovoScreen (ScreenKey key) {
  ScreenKey modifiers = key & ~SCR_KEY_CHAR_MASK;
  ScreenKey character = key & SCR_KEY_CHAR_MASK;
  char line[64];
  int length;

  {
    const char *name = specialKeyName(character);

    if (name) {
      length = snprintf(line, sizeof(line), "key %s", name);
    } else if (isSpecialKey(character)) {
      logMessage(LOG_WARNING, "key not supported in Dukovo's consoles: %04X", character);
      return 0;
    } else {
      length = snprintf(line, sizeof(line), "key U+%04X", (unsigned int)character);
    }
  }

  if (modifiers & (SCR_KEY_SHIFT | SCR_KEY_UPPER)) {
    length += snprintf(line + length, sizeof(line) - length, " shift");
  }

  if (modifiers & SCR_KEY_CONTROL) {
    length += snprintf(line + length, sizeof(line) - length, " ctrl");
  }

  if (modifiers & (SCR_KEY_ALT_LEFT | SCR_KEY_ALT_RIGHT)) {
    length += snprintf(line + length, sizeof(line) - length, " alt");
  }

  snprintf(line + length, sizeof(line) - length, "\n");
  return sendLine(line);
}

static int
switchVirtualTerminal_DukovoScreen (int vt) {
  char line[32];
  snprintf(line, sizeof(line), "switch %d\n", vt);
  return sendLine(line);
}

/* Only the shown console can be read: selecting another switches to it. */
static int
selectVirtualTerminal_DukovoScreen (int vt) {
  if ((vt == 0) || (vt == screenNumber)) return 1;
  return switchVirtualTerminal_DukovoScreen(vt);
}

static void
scr_initialize (MainScreen *main) {
  initializeRealScreen(main);
  main->base.poll = poll_DukovoScreen;
  main->base.refresh = refresh_DukovoScreen;
  main->base.describe = describe_DukovoScreen;
  main->base.readCharacters = readCharacters_DukovoScreen;
  main->base.insertKey = insertKey_DukovoScreen;
  main->base.currentVirtualTerminal = currentVirtualTerminal_DukovoScreen;
  main->base.selectVirtualTerminal = selectVirtualTerminal_DukovoScreen;
  main->base.switchVirtualTerminal = switchVirtualTerminal_DukovoScreen;
  main->construct = construct_DukovoScreen;
  main->destruct = destruct_DukovoScreen;
}
