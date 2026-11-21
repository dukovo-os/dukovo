go(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

go - build, run, test and check programs in Go, and manage their modules

# SYNOPSIS

*go* _command_ [_arguments_]++
*go help* [_command_ | _topic_]

# DESCRIPTION

*go* is the Go language's tool for a program's whole life: it finds the
packages a program imports, builds them, runs the program or its tests,
and keeps the list of modules the program needs. It comes in the bundle
*dev.go.go*, with *gofmt*(1), the compiler and the linker it runs, and
the standard library's source.

A program is built for Dukovo, to a single file. *go* needs no C
compiler: cgo is off (_CGO_ENABLED_ is 0), so a program that has C
code in it does not build here. Build such a program with *ob* on a
machine that has a C compiler.

# COMMANDS

*go build* [_packages_]
	Compile the packages and their imports. A main package is written
	as an executable, named for its folder or by *-o*. The first build
	compiles the standard library packages it needs into the build
	cache; later builds reuse them.

*go run* _package_ [_arguments_]
	Build a main package and run it with _arguments_.

*go test* [_packages_]
	Build and run the tests of the packages. *-short* leaves out the
	tests that take long, *-run* _pattern_ chooses tests by name, and
	*-v* prints every test as it runs.

*go vet* [_packages_]
	Look for mistakes the compiler does not report, such as a call to
	*Printf* with the wrong verbs.

*go fmt* [_packages_]
	Set out the source of the packages, by running *gofmt -l -w*.

*go mod* _subcommand_
	Work with the module: *go mod init* _path_ makes a _go.mod_ file,
	*go mod tidy* adds what imports need and removes the rest, *go mod
	vendor* copies the dependencies into the folder _vendor_.

*go get* _module_[@_version_]
	Add a dependency to _go.mod_, or change its version.

*go install* [_packages_]
	Build the packages and put the programs in _GOBIN_, which is
	_~/go/bin_ unless it is set.

*go env* [_name_...]
	Print Go's settings, all of them or those named, one value a line.
	*-w* _name_=_value_ keeps a setting in the user's file, _go/env_
	beneath the configuration folder.

*go version*
	Print the release and the system: *go version go1.27.4
	radix/amd64*.

*go clean* [*-cache* | *-modcache*]
	Remove what a build left in the folder, or empty the build cache
	or the module cache.

*go help*
	List every command and topic.

# ENVIRONMENT

_GOROOT_
	The folder of the Go tree. *go* finds it from where the program is,
	in the bundle's folder, so nothing needs to set it.

_GOCACHE_
	The build cache, _~/.cache/go-build_ unless set.

_GOPATH_
	Where downloaded modules and installed programs are, _~/go_ unless
	set.

_GOPROXY_
	Where modules are fetched from. The default is the public proxy,
	_https://proxy.golang.org_, then the module's own repository. Set
	it to *off* to refuse downloads.

_GOFLAGS_
	Flags every command takes, such as *-mod=vendor* to build from the
	folder _vendor_ without the network.

_GOTOOLCHAIN_
	*local*. A module whose _go.mod_ asks for a newer Go than this one
	fails with a message, as no release of Go from go.dev is built for
	Dukovo and *go* will not download one.

_CGO_ENABLED_
	*0*. Setting it to 1 makes *go* look for a C compiler, which
	Dukovo does not have.

*go env* prints these and the rest.

# FILES

_go.mod_, _go.sum_
	A module's name, its Go release and its dependencies with their
	checksums.

_~/.cache/go-build_
	The build cache.

_~/go/pkg/mod_
	Downloaded modules, read-only.

_~/go/bin_
	Programs *go install* writes.

_$GOROOT/go.env_
	The settings that apply until the user's file or the environment
	says otherwise.

# EXIT STATUS

0 on success. A command that fails exits with 1, and *go test* does so
when a test fails. *go run* exits with 1 when its program fails, and
prints the program's own status.

# LIMITS

The race detector (*-race*), plugins, *-buildmode=shared* and
*-buildmode=c-shared* are not here, and neither is cgo.

# SEE ALSO

*gofmt*(1), *bundle*(1), *ob*(1)

https://go.dev/doc
