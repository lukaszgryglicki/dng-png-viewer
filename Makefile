SHELL = /bin/sh
CARGO ?= cargo
JOBS ?= 4
TEST_THREADS ?= 4
PYTHON ?= python3
INSTALL_DIR ?= /data/scripts
BUILD_PATH = $(HOME)/.cargo/bin:$(PATH):/opt/homebrew/bin:/usr/local/bin
CHECK_REQUIREMENTS = PATH="$(BUILD_PATH)" CARGO="$(CARGO)" sh scripts/requirements.sh check

all: test build

build: release

requirements:
	@PATH="$(BUILD_PATH)" CARGO="$(CARGO)" sh scripts/requirements.sh install

target/.heif-config: .cargo/config.toml .cargo/heif.cmake
	@$(CHECK_REQUIREMENTS)
	PATH="$(BUILD_PATH)" $(CARGO) clean -p libheif-sys
	PATH="$(BUILD_PATH)" $(CARGO) clean --release -p libheif-sys
	PATH="$(BUILD_PATH)" CARGO_TARGET_DIR=target/static $(CARGO) clean --release -p libheif-sys
	@mkdir -p target
	@touch target/.heif-config

target/static/.sdl2-config: .cargo/config.toml .cargo/sdl2.cmake
	@$(CHECK_REQUIREMENTS)
	PATH="$(BUILD_PATH)" CARGO_TARGET_DIR=target/static $(CARGO) clean --release -p sdl2-sys
	@mkdir -p target/static
	@touch target/static/.sdl2-config

release: target/.heif-config
	@$(CHECK_REQUIREMENTS)
	PATH="$(BUILD_PATH)" $(CARGO) build --locked --release -j $(JOBS)

debug: target/.heif-config
	@$(CHECK_REQUIREMENTS)
	PATH="$(BUILD_PATH)" $(CARGO) build --locked -j $(JOBS)

static: target/.heif-config target/static/.sdl2-config
	@$(CHECK_REQUIREMENTS)
	PATH="$(BUILD_PATH)" CARGO_TARGET_DIR=target/static CMAKE_BUILD_PARALLEL_LEVEL=$(JOBS) \
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
	@$(CHECK_REQUIREMENTS)
	PATH="$(BUILD_PATH)" RUST_TEST_THREADS=$(TEST_THREADS) $(CARGO) test --locked -j $(JOBS)

test-display: release
	PATH="$(BUILD_PATH)" $(CARGO) test --locked --release --lib --no-run -j $(JOBS) --message-format=json > target/display-test-binaries.jsonl
	PATH="$(BUILD_PATH)" $(PYTHON) tests/display.py target/release/dng-png-viewer target/display-test-binaries.jsonl

fmt:
	PATH="$(BUILD_PATH)" $(CARGO) fmt --all

lint:
	@$(CHECK_REQUIREMENTS)
	PATH="$(BUILD_PATH)" $(CARGO) fmt --all -- --check
	PATH="$(BUILD_PATH)" $(CARGO) clippy --locked --all-targets -j $(JOBS) -- -D warnings

clean:
	PATH="$(BUILD_PATH)" $(CARGO) clean

.PHONY: all build requirements release debug static install test test-display fmt lint clean
