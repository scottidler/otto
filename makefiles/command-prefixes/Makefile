# Make allows `@`, `-`, and `+` combined, in either order, and repeated:
# `@` suppresses echo, `-` ignores the command's exit status, `+` always runs
# the recipe line even under `-n`. This fixture exists because the converter
# used to peel at most one leading prefix character, so `@-rm -rf dist` kept
# a literal `-` in the command with no `|| true`, and `-@echo hi` left a
# literal `@` for bash to choke on.

.PHONY: task

task:
	@-rm -rf dist
	-@echo hi
	+make -C sub
