mkiso(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

mkiso - make a disc image of a folder

# SYNOPSIS

*mkiso* [*--label* _label_] [*--udf*] [*--no-joliet*] [*--no-rock*] *-o* _file_ _folder_

# DESCRIPTION

mkiso writes an image of _folder_ and everything in it to _file_, for
*burn*(1) to write onto a disc, for a virtual machine's drive, or for
*mount*(8) (ADR 0449). The image is ISO 9660, which every system reads,
with:

- Rock Ridge, for POSIX systems: names of up to 255 bytes, modes, owners,
  times and symbolic links;
- Joliet, for Windows: Unicode names, cut to 64 characters;
- with *--udf*, a UDF 2.01 tree too, for DVD and Blu-ray players, which
  read only UDF.

The trees point at one copy of each file's bytes. A file of 4 GiB or
more is in pieces in ISO 9660's tree (its level 3), as *mkisofs* writes
it. ISO 9660's own names are made of upper-case letters, digits and
*\_*, eight and three long, and made unique, for readers of neither
extension. A folder deeper than
ISO 9660's eight levels is written where it is, as *mkisofs -D* writes
it; a reader of Rock Ridge, Joliet or UDF reads it.

The image is labelled _label_, or with the folder's name: in upper case
and at most 32 characters in ISO 9660's names, as given in Joliet's (cut
to 16) and UDF's (cut to 30). *volumed*(8) mounts a disc at its Joliet or
UDF label.

Sockets, pipes and devices are left out, and each is said on standard
error. Files are read as the image is written: one that changes size
meanwhile fails it. The image is written as _file.part_ and takes its
name once it is whole. Last, mkiso says how large it is:
```
/tmp/lab.iso: 1062 sectors, 2.2MB
```

# OPTIONS

*-o*, *--output* _file_
	Where the image goes.

*-V*, *--label* _label_
	The disc's label.

*--udf*
	A UDF tree beside ISO 9660's.

*--no-joliet*
	No Joliet names.

*--no-rock*
	No Rock Ridge.

# EXIT STATUS

0 when the image is written; 1 when the folder cannot be read, a name or
link is too long for the disc, a file changed as it was read, the image
cannot be written, or the command line has a mistake.

# SEE ALSO

*burn*(1), *mount*(8), *systools*(1), *volumed*(8)
