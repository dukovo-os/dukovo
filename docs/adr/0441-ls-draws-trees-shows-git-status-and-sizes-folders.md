<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0441. ls draws trees, shows git status and sizes folders

- Status: Accepted
- Date: 2026-11-09

## Context

This is the second part of the eza features Chase approved on
2026-11-09 (ADR 0440 has the first). eza can list a folder as a tree,
show each file's git status in the long listing, and show a folder's
size as what it holds. With GNU's `ls` each of these takes another
program: `tree`, `git status`, `du`. As in ADR 0440, eza is under the
EUPL-1.2, so we take its ideas and write our own code, and nothing
changes what `ls` prints unless it is asked for.

## Decision

Three additions to `ls`, in a second patch of the port
(`ports/uutils-coreutils/patches/0010`, code in `tree.rs`, `git.rs` and
`total_size.rs`).

- **`--tree`** lists each folder named with everything beneath it drawn
  under it (`├──`, `└──`, `│`), a name a line, and ends with a count of
  its folders and files, as `tree` does. With `-l`, each line keeps all
  its columns, lined up over the whole tree, and the lines go just
  before the name. Each folder's names are those `ls` lists there, in
  the same order: `.hidden` (ADR 0113), `-a`, `-I`, `-S`, `-t`, `-r`
  and `--group-directories-first` apply in it, and `.` and `..` are
  left out, as they are not beneath it. **`--level=N`** stops `N`
  folders down (1 is the folder's own names) and on its own means
  `--tree`. A link to a folder is followed only with `-L`, and a link
  back up is reported, as `-R` reports it, and not entered. `--tree`
  replaces `-R` and the columns of `-C`, `-x` and `-m`; `--dired` turns
  it off.
- **`--git`** adds a column to the long listing, after the time: two
  letters, as `git status --short`'s, for what is staged and what is
  changed and not staged. `N` new, `M` modified, `D` deleted, `R`
  renamed, `T` type changed, `U` in conflict, `I` ignored, `-`
  unchanged. A folder shows the most telling letter of what is in it on
  each side, `U` over `D` over `M` over `R` over `T` over `N`; ignored
  files inside say nothing of it. A listing with nothing in a
  repository has no column, and one outside a repository among others
  has blanks. A repository's own folder, named from outside it (`ls -ld
  --git repo`, or the top of a tree), shows what it holds. Its title
  under `--header` is Git.
- **`--total-size`** shows a folder's size, in the long listing and for
  `-S`, as the lengths of all the files beneath it, each file once
  however many names it has, and only on the folder's filesystem: what
  `du -s -x --apparent-size` counts, less the folders' own entries.
  `.` and `..` keep their own sizes,
  since `..` would count all of its parent. A folder that cannot be
  read counts as empty, without a message, as `ls` is not checking the
  tree.

`ls` asks `git` itself rather than reading repositories: one
`git rev-parse --show-toplevel` and one `git status --porcelain=v1 -z
--ignored=matching` for each repository, and a folder beneath one
already asked about uses its answer unless a `.git` of its own comes
between. Git runs with `--no-optional-locks`, so `ls` never writes to
the repository, and with `core.fsmonitor` off, since that setting names
a program git would start. Git's other settings and its check of who
owns a repository stay as they are: `ls --git` trusts a repository as
`git status` there does.

The field colours (ADR 0440) gain seven keys: `git-new`,
`git-modified` (for `M` and `T`), `git-deleted`, `git-renamed`,
`git-ignored`, `git-conflict` (a `-` takes `none`) and `tree`, for the
tree's lines. Nukesh's colors.toml `[ls]` has them under the same names,
and the themes change them: high-contrast bright, light without yellow,
colour-blind with blue and yellow for green and red.

## Consequences

- `ls --tree` and `ls -l --tree` replace `tree` for most uses; `tree`'s
  own options (`-P`, `-J`, HTML) are not copied.
- A tree is read whole before it is written, so that its columns line
  up; `ls --tree /` holds every name in memory and writes nothing until
  the end. `--level` keeps that small.
- `--total-size` reads every file beneath each folder listed, and a
  tree with it reads each one once for each folder above it. It is for
  finding what takes the space, not for every listing.
- `--git` needs `git` (ADR 0287). Without it, or outside a
  repository, there is no column and no message.
- The lab's `commands/ls-trees` test checks each flag, a folder it
  cannot read, the letters of a repository with each kind of change, and
  that a listing without the flags is GNU's; the `ls-views` boot test
  (ADR 0440's) checks a tree, its count, `--level`, a folder's total and
  a repository's letters at the console.

## Alternatives considered

- **Reading repositories in `ls`** (a git library). It would be faster
  than starting git, but `ls` would carry a second git that could
  disagree with the first, and git's own rules (ignore files, renames,
  submodules) are many. A library is also much more code in a program
  every shell starts.
- **Letters for a folder from its own entry only.** git reports an
  untracked or ignored folder as one entry, but a folder of tracked
  files has none; showing `--` for a folder of changed files would hide
  what `--git` is for.
- **`--total-size` counting blocks, as `du` does by default.** The size
  column shows lengths; a folder's size in blocks beside files' sizes
  in bytes would not add up.
- **Writing a tree as it is read.** It would start sooner, but the long
  listing's columns could not line up, and `-S` and `-t` need each
  folder whole anyway.
