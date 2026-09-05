# A substitution reference in target position: `$(SRCS:.c=.o)`. The rule's
# colon is the one AFTER the expansion, not the one inside it. Splitting on
# the first colon made the target `$(SRCS` and the dependency list
# `.c=.o): headers`, which contains an `=`, so the rule was dropped as a
# target-specific variable naming a target nobody wrote and its recipe was
# orphaned. `$(OBJS:%.o=%.d)` defeats the pattern-rule guard the same way.

SRCS = a.c b.c
OBJS = a.o b.o

$(SRCS:.c=.o): headers
	echo compile

$(OBJS:%.o=%.d): headers
	echo depend

headers:
	echo headers

build:
	echo build
