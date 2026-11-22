/* SPDX-FileCopyrightText: 2026 The Dukovo contributors */
/* SPDX-License-Identifier: Apache-2.0 OR MIT */

/*
 * The layout of a Radix user program: code, read-only data and data in
 * segments of their own, each starting on a page, so that every page has
 * one access and none is both writable and executable. Programs start at
 * 4 MiB, above the start-up page at 0x10000 (radix-startup(2)).
 *
 * Dukovo's ELF note (ADR 0610) says a program is written for Dukovo's
 * system calls: `exec` runs a program without it as a Linux program.
 * libradix writes the note, in the block that defines `_start`, so that
 * the programs that this script does not link have it too and the bytes
 * are in one place. The script only places it: first in the read-only
 * segment, so that it is in memory as well as in the file, and in a
 * PT_NOTE segment, which is where the kernel looks for it. Other notes
 * stay discarded.
 */

ENTRY(_start)

PHDRS
{
    text   PT_LOAD FLAGS(5);  /* read, execute */
    rodata PT_LOAD FLAGS(4);  /* read */
    data   PT_LOAD FLAGS(6);  /* read, write */
    note   PT_NOTE FLAGS(4);  /* read */
}

SECTIONS
{
    . = 0x400000;

    .text : ALIGN(4096)
    {
        *(.text._start)
        *(.text .text.*)
    } :text

    . = ALIGN(4096);
    .note.dukovo.abi : ALIGN(4)
    {
        KEEP(*(.note.dukovo.abi))
    } :rodata :note

    .rodata :
    {
        *(.rodata .rodata.*)
    } :rodata

    . = ALIGN(4096);
    .data : ALIGN(4096)
    {
        *(.data .data.*)
        *(.got .got.*)
    } :data

    .bss :
    {
        *(.bss .bss.*)
        *(COMMON)
    } :data

    /DISCARD/ :
    {
        *(.comment)
        *(.note .note.*)
        *(.eh_frame .eh_frame_hdr)
    }
}
