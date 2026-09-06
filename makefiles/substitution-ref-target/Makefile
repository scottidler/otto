# A substitution reference in target position: `$(SRCS:.c=.o)`. The rule's
# colon is the one AFTER the expansion, not the one inside it. Splitting on
# the first colon made the target `$(SRCS` and the dependency list
# `.c=.o): headers`, which contains an `=`, so the rule was dropped as a
# target-specific variable naming a target nobody wrote and its recipe was
# orphaned. `$(OBJS:%.o=%.d)` defeats the pattern-rule guard the same way.
#
# `$(BAD: headers` is the typo the same scan has to answer for: an expansion
# that is never closed swallows the rest of the line, colon included. It used
# to read as a line otto did not recognize, with its recipe then reported as
# belonging to no rule: two messages, neither naming the missing paren.

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

$(BAD: headers
	echo typo
