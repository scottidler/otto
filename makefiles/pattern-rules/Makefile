# A pattern rule is a template, not a task. Converting one used to produce a
# task literally named `%.o`, which then became the default task and failed
# the load with "unknown dependency %.c".

.DEFAULT_GOAL := build

%.o: %.c
	gcc -c $< -o $@

objects: %.o: %.c
	gcc -c $< -o $@

.SUFFIXES:

build:
	gcc -o app main.c
