SHELL = /bin/sh
CARGO ?= cargo
JOBS ?= 4
TEST_THREADS ?= 4
PYTHON ?= python3
INSTALL_DIR ?= /data/scripts

all: test build

build: release

target/.heif-config: .cargo/config.toml .cargo/heif.cmake
	$(CARGO) clean -p libheif-sys
	$(CARGO) clean --release -p libheif-sys
	CARGO_TARGET_DIR=target/static $(CARGO) clean --release -p libheif-sys
	@mkdir -p target
	@touch target/.heif-config

target/static/.sdl2-config: .cargo/config.toml .cargo/sdl2.cmake
	CARGO_TARGET_DIR=target/static $(CARGO) clean --release -p sdl2-sys
	@mkdir -p target/static
	@touch target/static/.sdl2-config

release: target/.heif-config
	$(CARGO) build --locked --release -j $(JOBS)

debug: target/.heif-config
	$(CARGO) build --locked -j $(JOBS)

static: target/.heif-config target/static/.sdl2-config
	CARGO_TARGET_DIR=target/static CMAKE_BUILD_PARALLEL_LEVEL=$(JOBS) \
		$(CARGO) build --locked --release --features static-sdl2 -j $(JOBS)
	@set -e; \
	binary=target/static/release/dng-png-viewer; \
	case "$$(uname -s)" in \
		Darwin) otool -L "$$binary" > target/static/runtime-libraries.txt ;; \
		*) ldd "$$binary" > target/static/runtime-libraries.txt ;; \
	esac; \
	if grep -Eq 'libSDL2|/SDL2.framework/' target/static/runtime-libraries.txt; then \
		echo "SDL2 was not linked statically" >&2; exit 1; \
	fi; \
	printf 'SDL2-static executable: %s\nNative codecs/OS graphics libraries/CRT remain dynamic; no SDL2 runtime package is required.\n' "$$binary"

install: release static
	install -m 755 target/release/dng-png-viewer dng-png-viewer
	install -m 755 target/static/release/dng-png-viewer dng-png-viewer.static
	mkdir -p "$(INSTALL_DIR)"
	install -m 755 dng-png-viewer.static "$(INSTALL_DIR)/dng-png-viewer"

test: target/.heif-config
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

.PHONY: all build release debug static install test test-display fmt lint clean
