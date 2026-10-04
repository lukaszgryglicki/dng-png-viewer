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
make test
make lint             # formatting and strict Clippy
```

Build/test parallelism defaults to four jobs/threads; override with `JOBS=...`
and `TEST_THREADS=...`. Runtime pixel processing uses available CPU threads;
`RAYON_NUM_THREADS=4` can bound it. The executable links the system SDL2 runtime;
it is not advertised as a dependency-free static binary. `make clean` removes
Cargo build artifacts.

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
| Left / Right | Previous / next image, stopping at the ends | Move the image left / right |
| Up / Down | Brighter / darker DR window; no action for ordinary images | Move the image up / down |
| Z | Center at one source pixel per display pixel | Recenter at 1:1 |
| X | Recenter and fit to screen | Return to fit-to-screen |
| Escape | Exit | Exit |

Panning moves the image by 64 pixels per keypress (including key repeat) and stops
at its edges. Smaller images stay centered. Z/X preserve the selected DR window;
loading another image resets it to that image's middle window. Native 1:1 uses
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
| 12 bits | 0-8, 1-9, 2-10, 3-11, 4-12 | 2-10 |
| 16 bits | 0-8 through 8-16 | 4-12 |

The middle shift is rounded down when necessary. **Up brightens** by selecting
one lower shift; **down darkens** by selecting one higher shift. These are
one-bit exposure steps, not bit masks that cycle gray levels.

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

The installed FreeBSD SDL2 used for development includes X11, KMSDRM and FreeBSD
keyboard support. Physical-console availability is hardware/session dependent;
an unavailable backend produces an explicit error rather than silently using
an invisible dummy display. The viewer does not change your desktop settings,
SDL installation or player configuration.

## Tests

`make test` exercises actual codecs, full 16-bit values and all windows, exact
1:1 pixels, resampling/alpha order, navigation/panning, orientation, CLI aliases,
directory ordering/deduplication/symlinks and headless analysis.

`make test-display` additionally needs Python 3, Xvfb, Xlib and XTest. It uses its
own isolated X server and synthetic fixtures, sends real keyboard events and
checks native framebuffer pixels and window geometry. It does not touch your
desktop or take over a physical VT. No image fixtures are kept in your photo
directories.

## License

Apache-2.0; see [LICENSE](LICENSE).
