# `$(TARGETS)` as a target name is a make variable reference this parser does
# not expand. It used to become a literal task named `$(TARGETS)` with no
# warning at all - `converter.rs` warns for `$` in a *dependency*, never in
# the target itself - and that unpronounceable name then became the default
# task.

TARGETS = build

$(TARGETS): dep
	echo building

dep:
	echo dep

build:
	echo build
