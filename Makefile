SHELL = /bin/sh
CARGO ?= cargo
JOBS ?= 4
TEST_THREADS ?= 4
PYTHON ?= python3

all: test build

build: release

release:
	$(CARGO) build --locked --release -j $(JOBS)

debug:
	$(CARGO) build --locked -j $(JOBS)

test:
	RUST_TEST_THREADS=$(TEST_THREADS) $(CARGO) test --locked -j $(JOBS)

test-display: release
	$(PYTHON) tests/display.py target/release/dng-png-viewer

fmt:
	$(CARGO) fmt --all

lint:
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --locked --all-targets -j $(JOBS) -- -D warnings

clean:
	$(CARGO) clean

.PHONY: all build release debug test test-display fmt lint clean
