<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0229. Access lists as security descriptors, and smbd fuzzed

- Status: Accepted
- Date: 2026-11-02

## Context

Phase 13f.3. A Windows PC shows who may do what to a shared file in its
Security tab, and `icacls` and Samba's `smbcacls` read and change the
same thing: the file's security descriptor (MS-DTYP 2.4.6), an owner, a
group and a DACL of ACEs, each a SID and an access mask. ADR 0228's smbd
gave a descriptor made from the modes and refused every change. Dukovo
keeps POSIX access lists as Linux does (ADR 0256), and those are what a
share's permissions are, so the Security tab should show them and change
them, and the names in it should be names, not numbers.

A DACL says more than a POSIX list can: denying ACEs, inheritance that
reaches only files or only folders, or only one level down, and SIDs of
domains and of Windows' own groups. A server that takes such a DACL and
keeps something else tells the PC it set what it did not. Windows sums
the ACEs that match a person; POSIX takes the first class that does
(owner, named user, groups, others).

smbd is the only Dukovo program that parses what any computer on the
network sends before it knows who sent it, so it is also fuzzed.

## Decision

- **The SIDs are Samba's for Unix's users and groups**: S-1-22-1-uid for
  a user and S-1-22-2-gid for a group, which PCs, Linux's cifs and
  Samba's tools know. Everyone (S-1-1-0) is the others' entry, and
  CREATOR OWNER (S-1-3-0) and CREATOR GROUP (S-1-3-1) a default list's
  owner's and group's entries.
- **Read** (QUERY_INFO): the owner's SID, the group's, and a DACL of
  allowing ACEs, one per entry of the access list, named entries and the
  group's limited by the mask; then a directory's default list as the
  same ACEs again, inheriting to files and folders and inherit-only
  (0x0b). `rwx` is FILE_ALL_ACCESS, which a PC shows as full control;
  otherwise read is FILE_GENERIC_READ, write FILE_GENERIC_WRITE with
  DELETE and DELETE_CHILD, and execute FILE_GENERIC_EXECUTE. Every ACE
  keeps the reading of attributes and the descriptor, so an entry with
  no permissions stays in the list, and the owner's has WRITE_DAC.
- **Set** (SET_INFO, given WRITE_DAC on the handle for a DACL, and
  WRITE_OWNER for an owner or group): an ACE gives the permissions its
  mask includes, read for reading data, write for writing or appending
  it, execute for running or traversing, and the generic rights their
  own. An ACE for the owner's SID gives the owner's entry, the group's
  the group's, Everyone's the others'; other users and groups get named
  entries; ACEs for one SID add up; the mask is the group class's
  permissions together. ACEs that inherit (to files, folders or both,
  one level or all) make a folder's default list in the same way;
  missing its owner's or group's entry it takes the folder's own, as
  setfacl(1) fills one in, and missing Everyone it gives the others
  nothing, as on a PC. A DACL with no ACE that inherits takes a folder's
  default list away. Refused, so that nothing else is kept in their
  place: denying ACEs and object ACEs (STATUS_NOT_SUPPORTED), a SID that
  names no one here (STATUS_NONE_MAPPED), a null DACL, which would let
  anyone do anything (STATUS_NOT_SUPPORTED), and an owner that is not a
  user or a group that is not a group. Everything is checked before
  anything changes; then the owner and group change with chown(2) and
  the lists with their attributes, and the kernel decides whether the
  person may: only root gives a file away, and only its owner changes
  its lists. A SACL is left as it is.
- **Names** come over IPC$'s lsarpc pipe (MS-LSAT): LsarOpenPolicy and
  LsarOpenPolicy2, LsarLookupSids and LsarLookupSids2, LsarLookupNames,
  LsarLookupNames2 and LsarLookupNames3, and LsarClose, in NDR. Users
  are named in the domain `Unix User` and groups in `Unix Group`, as
  Samba names them, so that alice the user and alice her group stay
  apart; a name alone is a well-known one, then a user's, then a
  group's. They come from the user and group databases, which everyone
  on the machine reads. A pipe has at most 16 policy handles, and a
  request's stub at most 1 MiB in all its fragments.
- **Fuzzed** as unit tests (smbd's fuzz.rs): valid messages bent at
  random, a bit, a byte, a 16- or 32-bit field set to an edge, cut,
  lengthened, repeated or shortened, from a fixed seed so that a failure
  is found again and names its round and input, are fed to the
  listener's sign-in (NEGOTIATE and both SESSION_SETUPs, made with
  smb2's NTLM client), to the worker (compounds on a read-only share
  and on IPC$, sealed with the session's key so that they reach its
  commands), to the srvsvc and lsarpc pipes, to the descriptors' reader
  and to the NTLM and SPNEGO readers. 4,000 rounds of each run with the
  unit tests; `SMBD_FUZZ=N` runs more and `SMBD_FUZZ_SEED` another
  seed. Nothing may panic, and the worker's answers must be sealed.
- **The access lists' model and their attributes** move from setfacl's
  and getfacl's code to a crate of their own, acl
  (userspace/programs/acl), which smbd and systools share.
- **Tested** by smbd's unit tests (the mapping both ways, the lsarpc
  pipe, and Samba's smbcacls against both halves on the loopback), and
  the `smbd` boot test, where the host's smbcacls reads alice's lists
  with names, adds root to a file's list and a folder's default list,
  and is refused a denying ACE and giving the file to root. smbcacls
  reaches only port 445, so both listen there: CI lets its tests bind
  it, and on a machine that does not the boot test leaves smbcacls out
  and says so.
- **Tested by Windows too**, in CI's Windows file sharing job
  (tools/windows/smbd.ps1): QEMU boots the `smbd` boot test's image on
  a Windows Server 2025 runner, without KVM, and Windows' own client
  maps its shares through a port QEMU forwards (`net use /TCPPORT`,
  since Windows' own server holds 445 there), copies files both ways,
  shows a file's list with `icacls`, adds root to it through
  SetNamedSecurityInfo, and is refused a denying entry, a write to the
  read-only share and a wrong password; smbdtest then checks what
  Windows did, as for Samba's client.

## Consequences

- The Security tab, `icacls` and `smbcacls` show and change a shared
  file's POSIX access lists, and what they are told was set is what the
  file has. The Security tab and `smbcacls` ask the server for names;
  `icacls` looks SIDs up on its own machine, which knows no Unix users,
  so it shows them as SIDs (S-1-22-1-1000) and cannot add one, even
  given as `*S-1-22-1-0` ("No mapping between account names and
  security IDs was done"). A program that gives the SID itself, through
  SetNamedSecurityInfo or .NET's access rules, can.
- Windows' sums and POSIX's classes differ: a person named in a list
  and also in its group gets the named entry alone on Dukovo, where a
  PC would add the two; and Everyone covers the others only, where a PC
  would give it to everyone. The tab shows each entry as it is.
- Deny entries, inheritance limits and Windows' own groups cannot be
  set from a PC; ADR 0256's lists have no place for them.
- A PC that finds a SID with no name (an account since removed) shows
  the SID, as it does for its own.
- **Not yet:** LsarQueryInformationPolicy and the domain's own SID, which the
  object picker of a domain-joined PC asks for; SACLs and auditing;
  SMB 3.1.1's POSIX extensions, which carry modes and owners without
  SIDs.
