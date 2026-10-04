# dng-png-viewer

A native Rust fullscreen image viewer for exploring **16-bit grayscale samples
through a movable 8-bit display window**. It runs under Xorg/XFCE or directly on
a supported KMS/DRM text console. Ordinary images use normal 8-bit color display.

The program decodes images, selects windows, scales/pans, draws and handles keys
itself. **No mpv, libmpv, FFmpeg, external viewer, helper process or temporary
rendered image is needed.** SDL2 supplies native window, display and input access.
Image files are read-only.

## Build

Requires Rust 1.89+, a C linker, `pkg-config` and SDL2 development files.

```sh
# FreeBSD
sudo pkg install sdl2 pkgconf

# Debian/Ubuntu
sudo apt install build-essential pkg-config libsdl2-dev

make                  # tests and stripped release build
make release          # target/release/dng-png-viewer
make debug            # target/debug/dng-png-viewer
make static           # target/static/release/dng-png-viewer (embeds SDL2)
make test
make lint             # formatting and strict Clippy
```

Build/test parallelism defaults to four jobs/threads; override with `JOBS=...`
and `TEST_THREADS=...`. Runtime pixel processing uses available CPU threads;
`RAYON_NUM_THREADS=4` can bound it. `make clean` removes Cargo build artifacts.

**`make static` embeds SDL2**, building its packaged source with CMake and
statically linking it along with the Rust code. It additionally requires CMake
and the graphics development dependencies (X11 and DRM/GBM/EGL for console
support). The usual SDL2 development package supplies the relevant dependency
set on Debian/Ubuntu; FreeBSD graphics development headers accompany the
installed graphics packages. Install CMake with `pkg install cmake` or
`apt install cmake` if it is missing.

This is an **SDL2-static build, not a fully static ELF**: native OS/graphics
libraries and GPU drivers still remain dynamic. It needs no SDL2 runtime
package. The target checks that the executable has no shared SDL2 dependency
and lists its other dependencies in `target/static/runtime-libraries.txt`.
A fully static graphics stack is not supplied: the installed EGL/GBM/DRM
libraries do not provide static archives, and removing those backends would
break the required console support. The normal release/debug builds continue
using the system SDL2.

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
Z/X preserve the selected DR window; loading another image resets it to that
image's full-range window, without extra highlight clipping. Native 1:1 uses
integer pixel alignment, including odd image/display dimensions. Fit mode
preserves aspect ratio with bilinear scaling and black borders.

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
Windowing happens **before** spatial interpolation, not after converting the
source to 8 bits.

The shift range is `0 .. max(code_bits - 8, 0)`, inclusive:

| Stored depth | Available labeled windows | Initial window |
| --- | --- | --- |
| 8 bits or less in a Gray16 file | 0-8 | 0-8 |
| 12 bits | 0-8, 1-9, 2-10, 3-11, 4-12 | 4-12 |
| 16 bits | 0-8 through 8-16 | 8-16 |

The highest (darkest) window is initially selected so every stored sample fits
without extra highlight clipping. The brighter windows still deliberately clip
highlights; their mapping is unchanged. **Up brightens** by selecting one lower
shift; **down darkens** by selecting one higher shift. These are one-bit exposure
steps, not bit masks that cycle gray levels.

A stretched PNG from `dng-monochrome` normally occupies all 16 code bits, even
when the original DNG had fewer meaningful camera stops. The PNG does not retain
enough information to reconstruct that original physical DR. The viewer also
reports minimum/maximum, occupied levels and `log2(maximum - minimum + 1)` in
analysis mode; none should be mistaken for a sensor SNR measurement. A constant
white image has 16 code bits but zero sample-span bits.

## Ordinary images and metadata

Supported formats: PNG, JPEG, GIF, BMP, TIFF, WebP, ICO, PBM/PGM/PPM/PAM/PNM,
TGA, QOI, farbfeld (`.ff`), Radiance HDR and OpenEXR. Animated formats display
their initial image; this is a still-image viewer, not an animation player.
DNG/other camera raws, AVIF, HEIC and JPEG XL are not supported.

Only decoded opaque 16-bit grayscale images enable DR scrolling, including
compatible Gray16 TIFF/PGM files. Eight-bit grayscale, RGB, RGBA, palette and
grayscale-with-alpha images use the ordinary display path. Higher-precision
color images are converted to 8-bit color; they do not enable DR scrolling.
Alpha is composited over black with premultiplied interpolation. Decoder-provided
EXIF orientation is applied before layout and statistics.

The viewer displays stored channel values; it is not an ICC-managed proofing
application or an HDR-monitor output pipeline. In particular, Gray16 code-window
viewing deliberately does not apply the PNG transfer tag.

## Options and large collections

Both single and double dashes work, including `-dir=/photos` and `--dir /photos`.

| Option | Meaning |
| --- | --- |
| `-dir DIRECTORY` | Recursively add supported image extensions; repeat freely |
| `-shuffle` | Shuffle the entire merged, deduplicated playlist |
| `-analyze` | Print one JSON object per image without opening a display |
| `-help`, `-h` | Help and controls |
| `-version`, `-V` | Program version |
| `--` | Treat subsequent arguments as filenames, even if they start with `-` |

Explicit files and directories retain their command-line order. Each directory's
discoveries are sorted by path; extension matching is case-insensitive. Repeated
canonical paths (including file symlinks) are included only once. Directory
symlinks are not recursively followed, avoiding loops. Missing inputs, traversal
errors and broken image symlinks are reported, not silently skipped. Explicit
filenames can omit an extension when the codec recognizes their contents.
Native non-UTF-8 paths remain usable; displayed/JSON names use replacement
characters for undecodable bytes.

If a shell glob would exceed the operating system's argument limit, **replace
the glob with `-dir`**. A program cannot fix `Argument list too long` after the
shell fails to start it. The viewer walks directories and keeps its playlist
in memory; it never forwards that playlist to another process.

Analysis reports are JSON Lines on stdout; diagnostics go to stderr. Analysis
needs no active display or video driver. It fully decodes each image to obtain
accurate sample statistics, so large collections still take time to inspect.

## Xorg/XFCE and the text console

With `DISPLAY` set, the viewer selects SDL's X11 backend. With only
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

`make test` exercises actual codecs, full 16-bit values and all windows, exact
1:1 pixels, resampling/alpha order, navigation/panning, orientation, CLI aliases,
directory ordering/deduplication/symlinks and headless analysis.
On FreeBSD it also checks console preflight, VT ownership/cleanup ordering,
startup races/failures, panic unwinding, termination signals and console keys
without accessing the physical console.

`make test-display` additionally needs Python 3, Xvfb, Xlib and XTest. It uses its
own isolated X server and synthetic fixtures, sends real keyboard events and
checks native framebuffer pixels and window geometry. It does not touch your
desktop or take over a physical VT. No image fixtures are kept in your photo
directories.
On FreeBSD, a real PTY also drives the console-input path against that isolated
display: arrow sequences, Z/X, Escape, SIGTERM and simulated VT-release signals
are exercised, and terminal-attribute restoration is checked. The test-only
entry point refuses physical consoles and is not included in the normal viewer.

## License

Apache-2.0; see [LICENSE](LICENSE).
