# A prerequisite that is a make expansion. Its name cannot be computed here,
# so the edge is dropped rather than emitted: an emitted `before: ['$(OBJS)']`
# named a task that cannot exist, and otto refused the whole run with
# `Task 'link' has unknown dependency '$(OBJS)'` at conversion exit 0.
#
# `headers`, a prerequisite that simply has no rule in this Makefile, keeps its
# edge on purpose: that name could be a real task in an included Makefile.

OBJS = a.o b.o

link: $(OBJS)
	echo linking

package: headers
	echo packaging
