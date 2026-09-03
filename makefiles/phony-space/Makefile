# `.PHONY : clean` (whitespace before the colon) is legal make; the parser
# used to match only the exact string `.PHONY:` and miss it, which produced
# two wrongs at once: "special target `.PHONY` is not converted", and then a
# false "`clean` is not `.PHONY` and has prerequisites" warning because
# `clean` no longer looked phony to the converter's file-rule heuristic.

.PHONY : clean

clean: dep
	rm -rf dist

dep:
	echo dep
