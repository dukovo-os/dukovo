/*
 * SPDX-FileCopyrightText: 2026 The Dukovo contributors
 * SPDX-License-Identifier: Apache-2.0 OR MIT
 *
 * The vDSO's layout (ADR 0616): one loadable segment, read and execute,
 * from the ELF header on, so that the image is mapped as it is, and the
 * clock page in the page below it. Nothing is writable: build.rs refuses
 * an image with a writable segment or a relocation.
 */

SECTIONS
{
	PROVIDE_HIDDEN(dukovo_clock_page = . - 4096);
	. = SIZEOF_HEADERS;

	.hash : { *(.hash) } :image
	.gnu.hash : { *(.gnu.hash) } :image
	.dynsym : { *(.dynsym) } :image
	.dynstr : { *(.dynstr) } :image
	.gnu.version : { *(.gnu.version) } :image
	.gnu.version_d : { *(.gnu.version_d) } :image
	.gnu.version_r : { *(.gnu.version_r) } :image
	.dynamic : { *(.dynamic) } :image :dynamic
	.rodata : { *(.rodata .rodata.*) } :image
	.eh_frame_hdr : { *(.eh_frame_hdr) } :image :unwind
	.eh_frame : { KEEP(*(.eh_frame)) } :image
	.text : { *(.text .text.*) } :image

	/DISCARD/ : { *(.comment) *(.note.GNU-stack) }
}

PHDRS
{
	image PT_LOAD FILEHDR PHDRS FLAGS(5);
	dynamic PT_DYNAMIC FLAGS(4);
	unwind PT_GNU_EH_FRAME FLAGS(4);
}
