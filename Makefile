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

static:
	CARGO_TARGET_DIR=target/static CMAKE_BUILD_PARALLEL_LEVEL=$(JOBS) \
		$(CARGO) build --locked --release --features static-sdl2 -j $(JOBS)
	@set -e; \
	binary=target/static/release/dng-png-viewer; \
	ldd "$$binary" > target/static/runtime-libraries.txt; \
	if grep -q libSDL2 target/static/runtime-libraries.txt; then \
		echo "SDL2 was not linked statically" >&2; exit 1; \
	fi; \
	printf 'SDL2-static executable: %s\nNative codecs/OS graphics libraries/CRT remain dynamic; no SDL2 runtime package is required.\n' "$$binary"

test:
	RUST_TEST_THREADS=$(TEST_THREADS) $(CARGO) test --locked -j $(JOBS)

test-display: release
	$(CARGO) test --locked --release --lib --no-run -j $(JOBS) --message-format=json > target/display-test-binaries.jsonl
	$(PYTHON) tests/display.py target/release/dng-png-viewer target/display-test-binaries.jsonl

fmt:
	$(CARGO) fmt --all

lint:
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --locked --all-targets -j $(JOBS) -- -D warnings

clean:
	$(CARGO) clean

.PHONY: all build release debug static test test-display fmt lint clean
