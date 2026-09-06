# A prerequisite that is a make expansion. Its name cannot be computed here,
# so the edge is dropped rather than emitted: an emitted `before: ['$(OBJS)']`
# named a task that cannot exist, and otto refused the whole run with
# `Task 'link' has unknown dependency '$(OBJS)'` at conversion exit 0.
#
# `headers`, a prerequisite that simply has no rule in this Makefile, is dropped
# too. It cannot become a task later: the converter refuses `include`, so it
# never reads another Makefile, and otto rejects a dangling edge file-wide.
#
# `deps.d`'s prerequisite is a SUBSTITUTION reference, so it carries a colon of
# its own. That colon used to make the rule read as a static pattern rule
# (`objs: %.o: %.c`) and the whole rule was skipped, recipe and all.

OBJS = a.o b.o

link: $(OBJS)
	echo linking

package: headers
	echo packaging

deps.d: $(OBJS:%.o=%.d)
	echo depend
