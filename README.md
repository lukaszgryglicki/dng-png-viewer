# dng-png-viewer

A native Rust fullscreen image viewer for exploring **16-bit grayscale samples
through a movable 8-bit display window**. It runs natively on macOS, under
Xorg/XFCE, or directly on a supported FreeBSD/Linux KMS/DRM text console.
Ordinary images use normal 8-bit color display.

The program decodes images, selects windows, scales/pans, draws and handles keys
itself. **No external viewer, mpv, FFmpeg executable, helper process or temporary
rendered image is needed.** SDL2 supplies native window, display and input access;
DNG, HEIC/HEIF/AVIF and JPEG2000 decoding also happens in-process. Image files
are read-only.

## Build

Requires Rust 1.89+, C/C++ compilers, CMake 3.22+, `pkg-config`, and SDL2, libde265 and
libaom development files. The codec tests also need x265 to generate HEIC fixtures.

```sh
# FreeBSD
sudo pkg install sdl2 pkgconf cmake libde265 aom x265

# Debian/Ubuntu
sudo apt install build-essential cmake pkg-config libsdl2-dev libde265-dev libaom-dev libx265-dev aom-tools libnuma-dev

# macOS (Xcode Command Line Tools and Homebrew)
brew install cmake pkgconf sdl2 libde265 aom x265

make                  # tests and stripped release build
make release          # target/release/dng-png-viewer
make debug            # target/debug/dng-png-viewer
make static           # target/static/release/dng-png-viewer (embeds SDL2)
make install          # build both; copy both to repo root, static to /data/scripts
make test
make lint             # formatting and strict Clippy
```

Build/test parallelism defaults to four jobs/threads; override with `JOBS=...`
and `TEST_THREADS=...`. Runtime pixel processing uses available CPU threads;
`RAYON_NUM_THREADS=4` can bound it.

`make install` builds both profiles and copies the normal release executable to
`./dng-png-viewer`, the SDL2-static executable to `./dng-png-viewer.static`, and
that static executable to `/data/scripts/dng-png-viewer`. Override the latter
directory with `INSTALL_DIR=/your/path`; it must be writable by the invoking
user. Both repository-root executables are ignored by Git.

`make release static install clean` performs the complete sequence. `make clean`
removes Cargo build artifacts but preserves both repository-root executables
and the installed copy. Omit `clean` to keep incremental build caches.

The default `bundled-heif` feature builds the packaged libheif source locally,
using libde265 for HEVC and libaom (or dav1d when installed) for AV1. Native codec
libraries remain dynamically linked; no system libheif is replaced. Only HEVC/AV1
codecs are enabled by `.cargo/heif.cmake`; the existing SDL2-specific toolchain
remains separate. JPEG2000 uses packaged OpenJPEG source built with the program;
no separate OpenJPEG development package is needed.
Ubuntu's native package exports can also require `aom-tools` (AOM CMake targets)
and `libnuma-dev` (x265's advertised linker dependency), as listed above.
Alpine's corresponding x265 linker dependency is supplied by `numactl-dev`.
To use a system libheif >=1.17 instead, install its development
package and build with `cargo build --locked --release --no-default-features`.
That library must have suitable HEVC/AV1 decoders: some FFmpeg-backed builds reject
12-bit grayscale HEIC. Missing codecs or unsupported files produce explicit errors.

Bundled builds require libde265 and libaom at configuration time; they no longer
silently omit HEVC/AV1 decoding when a development package is missing. The
decoder runs inside the viewer, not in an external image-viewing program.
The Makefile refreshes the HEIF dependency's debug, release and SDL2-static
caches when the HEIF configuration changes. For direct Cargo builds, after
installing previously missing codecs or changing `.cargo/heif.cmake`, refresh
the relevant cache before rebuilding:

```sh
cargo clean -p libheif-sys                 # debug
cargo clean --release -p libheif-sys       # normal release
CARGO_TARGET_DIR=target/static cargo clean --release -p libheif-sys
```

**`make static` embeds SDL2**, building its packaged source with CMake and
statically linking it along with the Rust code. It additionally requires
the graphics development dependencies (X11 and DRM/GBM/EGL for console
support). The usual SDL2 development package supplies the relevant dependency
set on Debian/Ubuntu; FreeBSD graphics development headers accompany the
installed graphics packages.

This is an **SDL2-static build, not a fully static ELF**: native codecs, OS/graphics
libraries and GPU drivers still remain dynamic. It needs no SDL2 runtime
package. The target checks that the executable has no shared SDL2 dependency
and lists its other dependencies in `target/static/runtime-libraries.txt`.
A fully static graphics stack is not supplied: the installed EGL/GBM/DRM
libraries do not provide static archives, and removing those backends would
break the required console support. The normal release/debug builds continue
using the system SDL2.

On macOS, `make static` also embeds SDL2 and checks dependencies with `otool`.
Apple system frameworks and native codec libraries remain dynamic. On Linux
the usual `make static` uses the native GNU toolchain, not musl. Builds targeting
Linux musl (with matching native dependencies) use mimalloc for Rust allocations
to avoid musl allocator contention; GNU/Linux, FreeBSD and macOS builds retain
their system allocator.
For native musl builds using shared SDL2/codec libraries, disable the default
static CRT: `RUSTFLAGS="-C target-feature=-crt-static" cargo build --locked`.

Cargo supplies the policy compatibility floor needed by bundled SDL2 with
CMake 4. The Makefile refreshes its static native cache when the SDL/Cargo
configuration changes, just as it does for HEIF.

The bundled build disables unused HIDAPI/game-controller/haptic support to avoid
an upstream SDL2 incompatibility with FreeBSD's USB headers. Keyboard input,
X11 and KMSDRM remain enabled; the native driver regression checks both display
backends. This configuration lives in `.cargo/sdl2.cmake` and also applies when
building directly with Cargo's `static-sdl2` feature.

## Run

```sh
./target/release/dng-png-viewer *.png
./target/release/dng-png-viewer -dir /photos/dng-mono
./target/release/dng-png-viewer first.png -dir /photos/one -dir /photos/two *.jpg -shuffle
./target/release/dng-png-viewer -preload 3 -brightness-step 0.2 *.DNG
./target/release/dng-png-viewer -preload 0 -dir /photos
./target/release/dng-png-viewer -analyze -dir /photos/dng-mono
./target/release/dng-png-viewer -help
```

The first image opens fullscreen, fitted to the display and centered. A short
on-screen status/key guide appears when loading or pressing a key and disappears
after four seconds. Decode errors remain visible and are also printed to stderr;
left/right can move past a bad image. An encountered decode error makes the final
exit status nonzero, even if subsequent images load successfully.

| Key | Fit-to-screen mode (default) | Native 1:1 mode |
| --- | --- | --- |
| Left / Right | Previous / next image, stopping at the ends | Pan toward the left / right of the image |
| Up / Down | Brighter / darker DR window; no action for ordinary images | Pan toward the top / bottom of the image |
| Z | Center at one source pixel per display pixel | Recenter at 1:1 |
| X | Recenter and fit to screen | Return to fit-to-screen |
| Escape / Q / Ctrl+C | Exit with cleanup | Exit with cleanup |

`Q` works with or without Shift. Ctrl+C is handled both as a focused-window
shortcut and as SIGINT from the launching terminal; SIGTERM also exits cleanly.
These all use the same cleanup path, not immediate process termination.

Panning moves the viewing area by 64 pixels in the arrow's direction per
keypress (including key repeat); the image itself moves in the opposite
direction. Panning stops at image edges, and smaller images stay centered.
Z/X preserve the selected DR window. Next/previous navigation also preserves it
when both images have the same available DR range, even across different formats
or dimensions. A different range, ordinary image or decode error resets it to the
next image's default; the first image starts at its full-range window. Native 1:1
uses integer pixel alignment, including odd image/display dimensions. Fit mode
preserves aspect ratio and black borders. Shrinking uses **area-weighted
averaging** of all covered source pixels: a 4:1 reduction averages the full 4x4
block, reducing noise and aliasing. `-downsample bilinear` restores the previous
four-pixel interpolation. Native 1:1 and bilinear enlargement are unchanged.

`-downsample full` is an alias for `area`. For single-pixel sampling, choose
`random`, `middle`, `NE`, `NW`, `SE` or `SW` (names are case-insensitive).
Middle selects the pixel containing the source footprint's center; the corners
select its first/last overlapping source rows and columns. Random uses a
repeatable selection of contributing pixels so brightness changes do not
reshuffle the samples. These modes retain more noise/aliasing than averaging.

## What the DR window means

For an opaque 16-bit grayscale image, the viewer preserves the exact decoded
unsigned samples, detects the largest sample and computes its **stored code
depth**. A maximum of 4095 uses 12 code bits; a maximum of 65535 uses 16.
This is not a measurement of camera/sensor stops, noise, scene contrast or
recoverable photographic information.

At shift `s`, each source sample becomes this display value:

```text
display = min(floor(sample / 2^s), 255)
```

Samples below `2^s` become black; values reaching/exceeding the selected window's
top become white. Highlights saturate rather than wrapping. There is no automatic
black subtraction, histogram stretch, gamma transformation or dithering.
Windowing happens **before** spatial averaging/interpolation, using the original
16-bit samples for every selected window. Downsampling does not change the
fractional brightness controls or apply a different tone/gamma curve.

The shift range is `0 .. max(code_bits - 8, 0)`, inclusive, with a default
step of **0.2 bits**:

| Stored depth | Available labeled windows | Initial window |
| --- | --- | --- |
| 8 bits or less in a Gray16 file | 0-8 | 0-8 |
| 12 bits | 0-8, 0.2-8.2, 0.4-8.4, ... through 4-12 | 4-12 |
| 16 bits | 0-8 through 8-16, including fifth-bit positions | 8-16 |

The highest (darkest) window is the default so every stored sample fits
without extra highlight clipping. The brighter windows still deliberately clip
highlights. **Up brightens** by reducing the shift; **down darkens** by increasing
it. For example, Up selects `8-16`, `7.8-15.8`, `7.6-15.6`, and so on.
`-brightness-step 0.25` selects quarter-bit steps; `-brightness-step 0.5` selects
half-bit steps; `-brightness-step 1` restores the old whole-bit controls.
Any finite step greater than zero and at most 8 is valid;
the endpoints are clamped. Fractional shifts are exposure scaling, not fractional
bit masks. Whole-bit positions produce exactly the same sample values as before.

A stretched PNG from `dng-monochrome` normally occupies all 16 code bits, even
when the original DNG had fewer meaningful camera stops. The PNG does not retain
enough information to reconstruct that original physical DR. The viewer also
reports minimum/maximum, occupied levels and `log2(maximum - minimum + 1)` in
analysis mode; none should be mistaken for a sensor SNR measurement. A constant
white image has 16 code bits but zero sample-span bits.

## Ordinary images and metadata

Supported formats: DNG, PNG, JPEG, HEIC/HEIF (`.heic`, `.heif`, `.hif`), AVIF,
JPEG2000 (`.jp2` containers and `.j2k`/`.j2c`/`.jpc` codestreams),
GIF, BMP, TIFF, WebP, ICO, PBM/PGM/PPM/PAM/PNM, TGA, QOI, farbfeld (`.ff`),
Radiance HDR and OpenEXR. Animated formats display their initial image;
HEIF containers display the primary image. This is a still-image viewer,
not an animation player. Other camera raw formats and JPEG XL are not supported.

Integer monochrome DNGs use full-resolution raw samples, not embedded previews.
The viewer applies the raw crop and orientation but does not subtract the black
level, stretch to the white level, or apply gamma. Color/CFA DNGs are developed
to ordinary color; floating-point DNGs also use the ordinary display path.

Only decoded opaque high-bit-depth integer grayscale images enable DR scrolling:
Gray16 PNG/TIFF/PGM, integer monochrome DNG, and compatible high-bit-depth
monochrome HEIC/AVIF/JPEG2000. Ten- and twelve-bit grayscale HEIF samples and
9-16-bit JPEG2000 samples retain their decoded code values in 16-bit storage,
without stretching them to 65535. Signed JPEG2000 samples are biased into the
corresponding unsigned code range. Unsupported JPEG2000 precision, subsampling
or color/alpha layouts produce explicit errors rather than guessed colors.
Eight-bit grayscale, RGB, RGBA, palette and grayscale-with-alpha images use the
ordinary display path. Higher-precision color images, including 10/12-bit HEIC,
are converted to 8-bit color; they do not enable DR scrolling.
Alpha is composited over black with premultiplied interpolation. Decoder-provided
EXIF orientation is applied before layout and statistics.

The viewer displays stored channel values; it is not an ICC-managed proofing
application or an HDR-monitor output pipeline. In particular, Gray16 code-window
viewing deliberately does not apply the PNG transfer tag.

## Options and large collections

Both single and double dashes work, including `-dir=/photos` and `--dir /photos`.
Positional inputs may be image files, directories, or **quoted globs**:

```sh
./target/release/dng-png-viewer /photos/album /more/photos -shuffle
./target/release/dng-png-viewer '/photos/name*.jpg'
./target/release/dng-png-viewer first.png '/photos/**/*.png' -dir /another/album
```

Directories are searched recursively without needing `-dir`. Quote wildcard
patterns so the viewer expands them internally instead of the shell exceeding
its argument limit before the program starts.

| Option | Meaning |
| --- | --- |
| `-dir DIRECTORY` | Recursively add supported image extensions; repeat freely |
| `-shuffle` | Shuffle once after all inputs are expanded and deduplicated |
| `-preload N` | Background-decode up to N images on each side; default 3, 0 disables preloading |
| `-brightness-step BITS` | DR adjustment per Up/Down keypress; default 0.2, valid range greater than 0 through 8 |
| `-downsample FILTER` | Shrinking filter: `area`/`full` (default average), `bilinear` (previous behavior), or one `random`, `middle`, `NE`, `NW`, `SE`, `SW` pixel |
| `-analyze` | Print one JSON object per image without opening a display |
| `-help`, `-h` | Help and controls |
| `-version`, `-V` | Program version |
| `--` | Treat subsequent arguments as inputs, even if they start with `-` |

Files, directories and globs retain their command-line order. Each directory's
discoveries and each glob's matches are sorted by path. Directory discovery
filters supported extensions case-insensitively; glob patterns are
case-sensitive and include hidden names. `*`, `?`, bracket classes and recursive
`**` patterns are supported. Glob-matched directories are recursively expanded
too. Existing literal paths take precedence, even if their names contain
wildcards; `-dir` always takes a literal directory.

Repeated canonical paths (including file symlinks) are included only once, at
their first occurrence. Nested directory symlinks are not traversed, avoiding
loops; a directory symlink supplied or matched as an input can be used as a
root. Missing inputs, malformed/unmatched globs, traversal errors and broken
image symlinks are reported, not silently skipped. Explicit and glob-matched
files can omit an extension when the codec recognizes their contents.
Native non-UTF-8 filenames and literal directory prefixes remain usable,
including filenames matched by globs; the wildcard portion of a pattern must
be valid UTF-8. Displayed/JSON names use replacement characters for undecodable
bytes.

The complete path list has **no application-level image-count limit**, including
collections of one million images. Paths must fit in available memory, but
building the list does not decode the images. `-shuffle` runs once on the final
list; navigation and prefetch use exactly that order for the whole viewing
session. The expanded list is never passed to a shell or another process.

Preloading keeps decoded pixels in a rolling cache of at most `2*N + 1` images,
bounded by the playlist ends. Moving right schedules the new rightmost neighbor
and drops images outside the new range; moving left works symmetrically.
The current image has its own loader, while a separate background worker loads
nearby images first using an independent single-thread pixel pool. Display/input
does not wait for preloads. Cached images can be selected immediately; cold
startup or navigation faster than decoding can still show a loading message.
Preload errors are shown only if that image is selected.

The cache costs decoded-image memory, not compressed-file size: a 60-megapixel
Gray16 image uses about 120 MB, so seven such images use about 840 MB, plus
in-flight decoding and rendering buffers. Use a smaller radius or `-preload 0`
on memory-constrained machines. Preloading is not used by `-analyze`.

Analysis reports are JSON Lines on stdout; diagnostics go to stderr. Analysis
needs no active display or video driver. It fully decodes each image to obtain
accurate sample statistics, so large collections still take time to inspect.

## Xorg/XFCE and the text console

On macOS the default backend is native SDL Cocoa, even if `DISPLAY` is set by
XQuartz or an SSH session. A graphical macOS login session is required; KMSDRM
is a FreeBSD/Linux backend, not a macOS console mode.

On FreeBSD/Linux, with `DISPLAY` set, the viewer selects SDL's X11 backend. With only
`WAYLAND_DISPLAY`, it selects Wayland. Without either, it selects **KMSDRM**.
An explicit `SDL_VIDEODRIVER` overrides automatic selection.
Select one driver only; comma-separated fallback lists are rejected so a failed
desktop connection cannot unexpectedly take over a physical console.

```sh
# From a terminal in XFCE/Xorg
./target/release/dng-png-viewer -dir /photos/dng-mono

# From an actual local text-console login with DRM/input access
SDL_VIDEODRIVER=KMSDRM ./target/release/dng-png-viewer -dir /photos/dng-mono
```

Direct-console mode requires SDL2 built with KMSDRM and suitable input support,
a KMS-capable graphics driver, access to the DRM/input/console devices, and an
available display not already owned by another graphics session. A generic SSH
terminal or terminal emulator is not itself a physical KMS console. Use your
system's normal active-console/session device permissions; do not make device
nodes world-writable or run the viewer as root to bypass them.

### FreeBSD console ownership and recovery

The viewer manages its own FreeBSD VT lifecycle rather than relying on SDL2's
unimplemented FreeBSD VT-switch callbacks. Before initializing video, it verifies
that its controlling terminal is the **active, foreground, ordinary text VT**.
An SSH/tmux/terminal-emulator PTY, an inactive VT, an existing graphics VT, or a
console owned by another application is rejected without opening graphics
devices. Run the viewer directly from a local text-console shell, not from tmux.
Tmux remains appropriate for the AI/build sessions supervising it.

It preserves terminal/keyboard/video modes and claims process-controlled VT
switching before creating SDL resources. The physical console keyboard stays
attached to the kernel; the viewer reads arrow/Z/X/Escape sequences directly.
This also works with an SDL2 build that has graphics but no evdev keyboard input.
Escape has a short (50 ms) disambiguation delay so an arrow's escape sequence
does not accidentally exit.

**Switching VTs exits the FreeBSD console viewer** instead of leaving it alive
with graphics ownership. Escape, Q, Ctrl+C, Ctrl+Z, hangup and termination signals
also take the cleanup path: SDL releases its graphics resources first, then
terminal/keyboard/video modes are restored, and only then is the pending VT
switch permitted. Startup errors and Rust unwinding restore modes too. X11/XFCE
behavior is unchanged.

No userspace program can guarantee cleanup after `kill -KILL`, a kernel/driver
hang or power loss. If necessary, use `kill -TERM <viewer-pid>` from another
terminal rather than SIGKILL. Do not restart Xorg while unsaved GUI work remains.

Physical-console availability is still hardware/session dependent. Lifecycle,
failure rollback, signal handling and PTY rejection are covered by isolated
tests; the real physical KMS display was deliberately **not retested on the
active workstation** after the reported disruption. An unavailable backend
produces an explicit error rather than an invisible dummy display. The viewer
does not change desktop configuration, SDL installation or player settings.

## Tests

`make test` exercises actual codecs (including synthetic DNG and 8/10/12-bit
HEIC/AVIF and 1-16-bit JPEG2000 fixtures), full 16-bit values and fractional windows, exact 1:1 pixels,
resampling/alpha order, navigation/panning, orientation, CLI aliases,
directory ordering/deduplication/symlinks and headless analysis. Worker tests
cover the sliding preload cache, eviction, radius zero, rapid navigation,
deferred errors and responsive foreground loading/shutdown.
On FreeBSD it also checks console preflight, VT ownership/cleanup ordering,
startup races/failures, panic unwinding, termination signals and console keys
without accessing the physical console.

`make test-display` additionally needs Python 3, Xvfb, Xlib and XTest. It uses its
own isolated X server and synthetic fixtures, sends real keyboard events and
checks native framebuffer pixels, window geometry and DR preservation/reset
through cached and cold navigation. It does not touch your
desktop or take over a physical VT. No image fixtures are kept in your photo
directories.
On FreeBSD, a real PTY also drives the console-input path against that isolated
display: arrow sequences, Z/X, Escape, SIGTERM and simulated VT-release signals
are exercised, and terminal-attribute restoration is checked. The test-only
entry point refuses physical consoles and is not included in the normal viewer.

## License

Apache-2.0; see [LICENSE](LICENSE).
