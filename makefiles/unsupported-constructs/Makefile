# Every construct below is one the converter cannot carry across faithfully.
# The fixture exists so the warnings stay pinned: silence here is the bug.

.DEFAULT_GOAL := build

NAME := example
VERSION := $(shell git describe --tags --always)
IMAGE := $(NAME):$(VERSION)
FLAGS += -race
export REGISTRY := registry.example.com
OBJECTS := $(wildcard *.c)

include shared.mk

ifeq ($(CI),true)
CACHE := off
endif

define banner
echo "=== banner"
endef

# Build the project
build: fmt
	@echo "Building $(NAME) $(VERSION)"
	docker build -t $(IMAGE) --build-arg REV=$(UNSET_BY_MAKEFILE) \
	  --label commit=$(GIT_SHA) .
	ps aux | awk '{print $$1}'

build: CFLAGS=-g

fmt check: ## Format the tree
	gofmt -l -w .

install:: build
	cp dist/$(NAME) /usr/local/bin

fmt:
	@echo "the last recipe for a target wins, as in make"
