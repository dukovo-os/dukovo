lazygit(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

lazygit - a terminal interface to git

# SYNOPSIS

*lazygit* [_options_] [*status* | *branch* | *log* | *stash*]

# DESCRIPTION

*lazygit* shows a git repository in panels: its status and changed
files, branches, commits, the stash, and what each change holds. Keys
stage and commit, switch, merge and rebase branches, pick and squash
commits, and push and pull. It runs *git*(1) for each of these, so what
it does is what git does. Press *?* in any panel for its keys, and *q* to
quit.

It starts in the repository of the current folder, or of the folder
*--path* names. The panel to start in may be given last: *status*,
*branch*, *log* or *stash*.

It comes in the bundle *io.github.jesseduffield.lazygit* of the Dukovo
catalog, and runs with the access of whoever runs it.

# OPTIONS

*-p*, *--path* _folder_
	The repository to open, as *--work-tree*=_folder_ *--git-dir*=_folder_/.git.

*-f*, *--filter* _path_
	Show only the commits, reflog entries and stashes that change _path_,
	as *git log -- *_path_ does.

*-w*, *--work-tree* _folder_, *-g*, *--git-dir* _folder_
	git's own *--work-tree* and *--git-dir*.

*-sm*, *--screen-mode* *normal* | *half* | *full*
	How much of the screen the focused panel takes at the start.

*-c*, *--config*
	Print the default settings and exit.

*-cd*, *--print-config-dir*
	Print the folder of its settings and exit.

*-ucd*, *--use-config-dir* _folder_
	Read its settings from _folder_.

*-ucf*, *--use-config-file* _file_[,_file_...]
	Read its settings from these files, later ones over earlier ones.

*-d*, *--debug*, *-l*, *--logs*
	Write a log of what it does; with *--logs*, in another terminal,
	follow that log.

*-v*, *--version*
	Print its version and exit.

# FILES

_~/.config/lazygit/config.yml_
	Its settings, unless _XDG_CONFIG_HOME_ names another folder for
	them. *lazygit --config* prints every setting with its default.

# ON DUKOVO

A file another program changes while *lazygit* runs shows when the panel
is next focused or *R* refreshes it, as Dukovo does not tell programs of
changed files. There is no clipboard to copy to: Dukovo has no display
server.

# SEE ALSO

*git*(1), https://github.com/jesseduffield/lazygit/blob/master/docs/Config.md
