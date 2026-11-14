// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// The parser of what a program writes to the terminal: bytes in, and the
// characters, controls and sequences they make handed to a handler. It is
// the state machine of DEC's VT500 terminals (Paul Williams' chart of it),
// with UTF-8 in front, because the machine's programs write UTF-8.
//
// The handler has five methods, and the parser calls nothing else:
//
//   print(codePoint)                       a character to draw
//   control(code)                          a C0 control to carry out
//   escape(intermediates, final)           ESC, then `intermediates`, then `final`
//   csi(prefix, params, intermediates, final)
//   osc(text)                              the whole string, whichever way it ended
//
// `final`, `prefix` and `intermediates` are strings. `params` is an array of
// numbers that the parser reuses for the next sequence, so a handler reads
// it at once and keeps no reference. A missing parameter is 0, as `CSI ; m`
// has two, both 0; the handler decides what 0 means for each sequence.
//
// Everything a hostile program could grow is bounded: a sequence has at
// most MAX_PARAMS parameters, each at most MAX_PARAM_VALUE, and an OSC
// string keeps at most MAX_OSC_LENGTH UTF-16 units. A sequence over a limit
// is ignored whole, never cut.
//
// Two choices keep the terminal safe against text that others wrote and
// that a program only shows (file names, mail, logs):
//
// - The C1 controls (U+0080 to U+009F, which UTF-8 can carry) do nothing
//   and are not drawn. In a UTF-8 terminal an 8-bit CSI would let any text
//   that holds U+009B run a sequence, past the filters that look for ESC.
// - A colon in a CSI sequence ends it as ignored, as the original chart has
//   it, so a sequence that asks for something this terminal does not know
//   does nothing instead of something else.

export const MAX_PARAMS = 32;
export const MAX_PARAM_VALUE = 65535;
export const MAX_OSC_LENGTH = 2048;
const MAX_INTERMEDIATES = 2;

const REPLACEMENT = 0xfffd;

/**
 * The states of the machine. DCS, SOS, PM and APC strings are all dropped
 * up to their end, so they share STRING.
 */
export const State = Object.freeze({
  GROUND: 0,
  ESCAPE: 1,
  ESCAPE_INTERMEDIATE: 2,
  CSI_ENTRY: 3,
  CSI_PARAM: 4,
  CSI_INTERMEDIATE: 5,
  CSI_IGNORE: 6,
  OSC: 7,
  STRING: 8,
});

export class Parser {
  /** Makes a parser that calls `handler` (see above) for what it reads. */
  constructor(handler) {
    this.handler = handler;
    this.params = [];
    this.reset();
  }

  /** Forgets any half-read sequence or character and goes back to the ground state. */
  reset() {
    this.state = State.GROUND;
    this.#clear();
    this.#resetDecoder();
  }

  /** Reads `bytes`, a Uint8Array. A sequence or a character may end in a later call. */
  write(bytes) {
    for (let i = 0; i < bytes.length; i++) {
      const byte = bytes[i];
      if (this.needed !== 0 || byte >= 0x80) {
        this.#decode(byte);
      } else if (this.state === State.GROUND && byte >= 0x20 && byte !== 0x7f) {
        this.handler.print(byte);
      } else {
        this.feed(byte);
      }
    }
  }

  // UTF-8 as the WHATWG decoder reads it: a byte that cannot continue a
  // character gives one U+FFFD for what came before and then starts over,
  // so one bad byte never costs the good ones after it. Overlong forms,
  // surrogates and values past U+10FFFF are refused by the bounds on the
  // second byte.
  #decode(byte) {
    if (this.needed === 0) {
      if (byte >= 0xc2 && byte <= 0xdf) {
        this.needed = 1;
        this.codePoint = byte & 0x1f;
      } else if (byte >= 0xe0 && byte <= 0xef) {
        if (byte === 0xe0) this.lower = 0xa0;
        if (byte === 0xed) this.upper = 0x9f;
        this.needed = 2;
        this.codePoint = byte & 0x0f;
      } else if (byte >= 0xf0 && byte <= 0xf4) {
        if (byte === 0xf0) this.lower = 0x90;
        if (byte === 0xf4) this.upper = 0x8f;
        this.needed = 3;
        this.codePoint = byte & 0x07;
      } else {
        this.feed(REPLACEMENT);
      }
      return;
    }
    if (byte < this.lower || byte > this.upper) {
      this.#resetDecoder();
      this.feed(REPLACEMENT);
      if (byte < 0x80) this.feed(byte);
      else this.#decode(byte);
      return;
    }
    this.lower = 0x80;
    this.upper = 0xbf;
    this.codePoint = (this.codePoint << 6) | (byte & 0x3f);
    this.seen += 1;
    if (this.seen === this.needed) {
      const codePoint = this.codePoint;
      this.#resetDecoder();
      this.feed(codePoint);
    }
  }

  #resetDecoder() {
    this.needed = 0;
    this.seen = 0;
    this.codePoint = 0;
    this.lower = 0x80;
    this.upper = 0xbf;
  }

  #clear() {
    this.params.length = 0;
    this.value = 0;
    this.hasParams = false;
    this.prefix = '';
    this.intermediates = '';
    this.overflow = false;
    this.text = '';
  }

  /**
   * Takes one code point through the machine. `write` calls it; it is public
   * so that a test can drive a state with a character that is not a byte.
   */
  feed(cp) {
    // These act the same in every state. ESC starts a sequence (and ends an
    // OSC string, which a program may end with ESC \), CAN and SUB cancel
    // one, and DEL and the C1 controls are dropped.
    if (cp < 0x20) {
      if (cp === 0x1b) {
        if (this.state === State.OSC) this.#endOsc();
        this.#clear();
        this.state = State.ESCAPE;
        return;
      }
      if (cp === 0x18 || cp === 0x1a) {
        this.state = State.GROUND;
        return;
      }
    } else if (cp >= 0x7f && cp <= 0x9f) {
      return;
    }
    switch (this.state) {
      case State.GROUND:
        if (cp < 0x20) this.handler.control(cp);
        else this.handler.print(cp);
        return;
      case State.ESCAPE:
        this.#escape(cp);
        return;
      case State.ESCAPE_INTERMEDIATE:
        this.#escapeIntermediate(cp);
        return;
      case State.CSI_ENTRY:
      case State.CSI_PARAM:
        this.#csiParam(cp);
        return;
      case State.CSI_INTERMEDIATE:
        this.#csiIntermediate(cp);
        return;
      case State.CSI_IGNORE:
        this.#csiIgnore(cp);
        return;
      case State.OSC:
        this.#osc(cp);
        return;
      default:
        // STRING: nothing in it matters, and only the cases above end it.
    }
  }

  // A code point past ASCII in the middle of a sequence means the sequence
  // was cut short: it is dropped, and the character is text again.
  #abort(cp) {
    this.state = State.GROUND;
    this.feed(cp);
  }

  #collect(cp) {
    if (this.intermediates.length >= MAX_INTERMEDIATES) this.overflow = true;
    else this.intermediates += String.fromCharCode(cp);
  }

  #escape(cp) {
    if (cp < 0x20) {
      this.handler.control(cp);
    } else if (cp < 0x30) {
      this.#collect(cp);
      this.state = State.ESCAPE_INTERMEDIATE;
    } else if (cp > 0x7e) {
      this.#abort(cp);
    } else if (cp === 0x5b) {
      this.state = State.CSI_ENTRY;
    } else if (cp === 0x5d) {
      this.state = State.OSC;
    } else if (cp === 0x50 || cp === 0x58 || cp === 0x5e || cp === 0x5f) {
      this.state = State.STRING;
    } else {
      this.state = State.GROUND;
      this.handler.escape('', String.fromCharCode(cp));
    }
  }

  #escapeIntermediate(cp) {
    if (cp < 0x20) {
      this.handler.control(cp);
    } else if (cp < 0x30) {
      this.#collect(cp);
    } else if (cp > 0x7e) {
      this.#abort(cp);
    } else {
      this.state = State.GROUND;
      if (!this.overflow) this.handler.escape(this.intermediates, String.fromCharCode(cp));
    }
  }

  #csiParam(cp) {
    if (cp < 0x20) {
      this.handler.control(cp);
    } else if (cp < 0x30) {
      this.#collect(cp);
      this.state = State.CSI_INTERMEDIATE;
    } else if (cp <= 0x39) {
      this.hasParams = true;
      this.value = Math.min(this.value * 10 + cp - 0x30, MAX_PARAM_VALUE);
      this.state = State.CSI_PARAM;
    } else if (cp === 0x3b) {
      this.#separator();
    } else if (cp === 0x3a) {
      this.state = State.CSI_IGNORE;
    } else if (cp <= 0x3f) {
      // A marker (< = > ?) belongs only before the first parameter.
      if (this.state === State.CSI_ENTRY) {
        this.prefix = String.fromCharCode(cp);
        this.state = State.CSI_PARAM;
      } else {
        this.state = State.CSI_IGNORE;
      }
    } else if (cp <= 0x7e) {
      this.#dispatchCsi(cp);
    } else {
      this.#abort(cp);
    }
  }

  #separator() {
    this.state = State.CSI_PARAM;
    this.hasParams = true;
    if (this.params.length >= MAX_PARAMS) {
      this.state = State.CSI_IGNORE;
      return;
    }
    this.params.push(this.value);
    this.value = 0;
  }

  #csiIntermediate(cp) {
    if (cp < 0x20) {
      this.handler.control(cp);
    } else if (cp < 0x30) {
      this.#collect(cp);
    } else if (cp <= 0x3f) {
      this.state = State.CSI_IGNORE;
    } else if (cp <= 0x7e) {
      this.#dispatchCsi(cp);
    } else {
      this.#abort(cp);
    }
  }

  #csiIgnore(cp) {
    if (cp < 0x20) this.handler.control(cp);
    else if (cp > 0x7e) this.#abort(cp);
    else if (cp >= 0x40) this.state = State.GROUND;
  }

  #dispatchCsi(cp) {
    this.state = State.GROUND;
    if (this.overflow) return;
    if (this.hasParams) {
      if (this.params.length >= MAX_PARAMS) return;
      this.params.push(this.value);
    }
    this.handler.csi(this.prefix, this.params, this.intermediates, String.fromCharCode(cp));
  }

  // The string of an OSC ends with BEL, as xterm has it, or ST (ESC \).
  #osc(cp) {
    if (cp === 0x07) {
      this.#endOsc();
      this.state = State.GROUND;
    } else if (cp >= 0x20) {
      if (this.text.length >= MAX_OSC_LENGTH) this.overflow = true;
      else this.text += String.fromCodePoint(cp);
    }
  }

  #endOsc() {
    if (!this.overflow) this.handler.osc(this.text);
  }
}
