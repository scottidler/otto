.DEFAULT_GOAL := build

# go source files, ignore vendor directory
SRC = $(shell find . -type f -name '*.go' -not -path "./vendor/*")

VERSION := $(shell git describe --tags --always --dirty)
BUILD := `git rev-parse HEAD`

LDFLAGS=-ldflags "-X=main.Version=$(VERSION) -X=main.Build=$(BUILD)"

.PHONY: all
all: unit-test type-check lint build

.PHONY: build
build:
	mkdir -p dist
	go build -o dist/example cmd/example/main.go

.PHONY: run
run: build
	./dist/example 2> err.log

# Testing related targets
.PHONY: unit-test
unit-test:
	go test -cover -coverprofile=coverage.txt ./...

.PHONY: test
test: unit-test

.PHONY: coverage
coverage: unit-test
	go tool cover -html=coverage.txt

.PHONY: fmt
fmt:
	gofmt -l -w $(SRC)

.PHONY: type-check
type-check:
	go run honnef.co/go/tools/cmd/staticcheck@v0.5.1 ./...

.PHONY: lint
lint: fmt

# Clean
.PHONY: clean
clean:
	rm -rf dist/
	rm -rf .coverage
