# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""Writes BRLTTY's make dependencies: absdeps.mk at the top of the source
tree and reldeps.mk in each directory that has a Makefile.in.

BRLTTY's makefiles include both. Its release archives carry them, made by
its Tools/gendeps, a Tcl script; its git tree, which ob builds from, does
not, and the build machine has no Tcl. This does the same: each C, C++
or yacc source file depends on the files it includes with #include "...",
found beside it, then in Programs, Headers and the top, and on what those
include in turn. A generated file (NAME.auto.EXT, NAME.tab.c and
brlapi_constants.h) is named where it is included, and absdeps.mk says to
make it in its own directory. A file made from NAME.in by configure, as
config.h is, is in the build tree; every other file in the source tree.

Usage: python3 -I gendeps.py SOURCE
"""

import os
import re
import sys

INCLUDE = re.compile(r'^ *# *include *"([^"]*)"')
SOURCE_EXTENSIONS = (".c", ".cc", ".y")


def generated(path):
    """Whether make generates `path` during the build."""
    name = os.path.basename(path)
    root, _ = os.path.splitext(name)
    return (os.path.splitext(root)[1] == ".auto" or name.endswith(".tab.c")
            or name == "brlapi_constants.h")


def made_from(path):
    """The NAME.in that configure makes `path` from, if there is one:
    forbuild.h is made from config.h.in, as config.h is."""
    directory, name = os.path.split(path)
    name = re.sub(r"^forbuild(\.)", r"config\1", name)
    source = os.path.join(directory, name + ".in")
    return source if os.path.exists(source) else None


def exists(path):
    return os.path.exists(path) or made_from(path) is not None


def build_directories(top="."):
    """Each directory under `top` with a Makefile.in, in walking order;
    symbolic links are not followed."""
    found = []
    for directory, subdirectories, files in os.walk(top):
        subdirectories[:] = sorted(
            d for d in subdirectories if not os.path.islink(os.path.join(directory, d)))
        if "Makefile.in" in files:
            found.append(os.path.normpath(directory))
    return found


class Dependencies:
    """Each file's dependencies, itself first, worked out once."""

    def __init__(self):
        self.known = {}

    def of(self, path):
        if path in self.known:
            return self.known[path]
        dependencies = [path]
        self.known[path] = dependencies
        with open(made_from(path) or path, encoding="latin-1") as stream:
            includes = [m.group(1) for m in map(INCLUDE.match, stream) if m]
        directory = os.path.dirname(path)
        for include in includes:
            if generated(include):
                if "/" not in include:
                    include = os.path.join(directory, include)
                dependencies.append(include)
                continue
            for location in (directory, "Programs", "Headers", ""):
                candidate = os.path.normpath(os.path.join(location, include))
                if exists(candidate):
                    for dependency in self.of(candidate):
                        if dependency not in dependencies:
                            dependencies.append(dependency)
                    break
            else:
                print(f"gendeps: missing dependency: {path} includes {include}",
                      file=sys.stderr)
        return dependencies


def main():
    os.chdir(sys.argv[1])
    directories = build_directories()
    dependencies = Dependencies()
    sources = {}
    for directory in directories:
        names = sorted(n for n in os.listdir(directory) if n.endswith(SOURCE_EXTENSIONS))
        paths = [os.path.normpath(os.path.join(directory, n)) for n in names]
        sources[directory] = [p for p in paths if not generated(p)]
        for path in sources[directory]:
            dependencies.of(path)

    wanted = []
    objects = []
    with open("absdeps.mk", "w") as absolute:
        for directory in sorted(directories):
            with open(os.path.join(directory, "reldeps.mk"), "w") as relative:
                for source in sources[directory]:
                    root, extension = os.path.splitext(source)
                    object_file = root + (".tab.c" if extension == ".y" else ".$O")
                    object_name = os.path.basename(object_file)
                    absolute.write(f"# Dependencies for {object_file}:\n")
                    relative.write(f"# Dependencies for {object_name}:\n")
                    objects.append(object_file)
                    for dependency in dependencies.of(source):
                        made = generated(dependency)
                        if made:
                            wanted.append(dependency)
                        tree = "BLD" if made or made_from(dependency) else "SRC"
                        absolute.write(f"$(BLD_TOP){object_file}: $({tree}_TOP){dependency}\n")
                        if os.path.dirname(dependency) == directory or (
                                directory == "." and "/" not in dependency):
                            prefix = "" if tree == "BLD" else f"$({tree}_DIR)/"
                            relative.write(f"{object_name}: {prefix}{os.path.basename(dependency)}\n")
                        else:
                            relative.write(f"{object_name}: $({tree}_TOP){dependency}\n")
                    absolute.write("\tcd $(@D) && $(MAKE) $(@F)\n\n")
                    relative.write("\n")
        wanted = sorted(set(w for w in wanted if w not in objects))
        if wanted:
            absolute.write("# Generated dependencies:\n")
            for dependency in wanted:
                absolute.write(f"$(BLD_TOP){dependency}:\n")
                absolute.write("\tcd $(@D) && $(MAKE) $(@F)\n")
            absolute.write("\n")


if __name__ == "__main__":
    main()
