#!/usr/bin/env python3
"""Native X11 integration tests; only an isolated Xvfb and synthetic files are used."""

import array
import contextlib
import ctypes as c
import ctypes.util
import hashlib
import math
import os
from pathlib import Path
import select
import shutil
import struct
import subprocess
import sys
import tempfile
import time
import zlib


WIDTH, HEIGHT = 1024, 768


def wait_for(check, description, timeout=30):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        value = check()
        if value:
            return value
        time.sleep(0.025)
    raise AssertionError(f"Timed out: {description}")


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


class XImage(c.Structure):
    _fields_ = [
        ("width", c.c_int), ("height", c.c_int), ("xoffset", c.c_int),
        ("format", c.c_int), ("data", c.c_void_p), ("byte_order", c.c_int),
        ("bitmap_unit", c.c_int), ("bitmap_bit_order", c.c_int),
        ("bitmap_pad", c.c_int), ("depth", c.c_int),
        ("bytes_per_line", c.c_int), ("bits_per_pixel", c.c_int),
        ("red_mask", c.c_ulong), ("green_mask", c.c_ulong),
        ("blue_mask", c.c_ulong),
    ]


class XError(c.Structure):
    _fields_ = [
        ("type", c.c_int), ("display", c.c_void_p), ("resource", c.c_ulong),
        ("serial", c.c_ulong), ("code", c.c_ubyte),
        ("request", c.c_ubyte), ("minor", c.c_ubyte),
    ]


class Display:
    def __init__(self, name):
        self.x = c.CDLL(ctypes.util.find_library("X11") or "libX11.so")
        self.xt = c.CDLL(ctypes.util.find_library("Xtst") or "libXtst.so")
        signatures = {
            "XOpenDisplay": (c.c_void_p, [c.c_char_p]),
            "XDefaultRootWindow": (c.c_ulong, [c.c_void_p]),
            "XQueryTree": (c.c_int, [c.c_void_p, c.c_ulong, c.POINTER(c.c_ulong), c.POINTER(c.c_ulong), c.POINTER(c.POINTER(c.c_ulong)), c.POINTER(c.c_uint)]),
            "XFetchName": (c.c_int, [c.c_void_p, c.c_ulong, c.POINTER(c.c_char_p)]),
            "XFree": (c.c_int, [c.c_void_p]),
            "XGetGeometry": (c.c_int, [c.c_void_p, c.c_ulong, c.POINTER(c.c_ulong), c.POINTER(c.c_int), c.POINTER(c.c_int), c.POINTER(c.c_uint), c.POINTER(c.c_uint), c.POINTER(c.c_uint), c.POINTER(c.c_uint)]),
            "XGetImage": (c.POINTER(XImage), [c.c_void_p, c.c_ulong, c.c_int, c.c_int, c.c_uint, c.c_uint, c.c_ulong, c.c_int]),
            "XDestroyImage": (c.c_int, [c.POINTER(XImage)]),
            "XSetInputFocus": (c.c_int, [c.c_void_p, c.c_ulong, c.c_int, c.c_ulong]),
            "XStringToKeysym": (c.c_ulong, [c.c_char_p]),
            "XKeysymToKeycode": (c.c_ubyte, [c.c_void_p, c.c_ulong]),
            "XSync": (c.c_int, [c.c_void_p, c.c_int]),
            "XCloseDisplay": (c.c_int, [c.c_void_p]),
        }
        for name_, (result, arguments) in signatures.items():
            function = getattr(self.x, name_)
            function.restype, function.argtypes = result, arguments
        self.xt.XTestFakeKeyEvent.argtypes = [c.c_void_p, c.c_uint, c.c_int, c.c_ulong]
        self.xt.XTestFakeKeyEvent.restype = c.c_int
        self.errors = []
        handler = c.CFUNCTYPE(c.c_int, c.c_void_p, c.POINTER(XError))

        @handler
        def on_error(_display, event):
            self.errors.append((event.contents.code, event.contents.resource))
            return 0

        self.on_error = on_error
        self.x.XSetErrorHandler.argtypes = [handler]
        self.x.XSetErrorHandler(self.on_error)
        self.display = self.x.XOpenDisplay(name.encode())
        if not self.display:
            raise RuntimeError(f"Cannot connect to isolated X server {name}")
        self.root = self.x.XDefaultRootWindow(self.display)

    def close(self):
        self.x.XCloseDisplay(self.display)

    def title(self, window):
        value = c.c_char_p()
        if not self.x.XFetchName(self.display, window, c.byref(value)):
            return ""
        try:
            return value.value.decode("utf-8", errors="replace") if value.value else ""
        finally:
            self.x.XFree(value)

    def viewer_window(self):
        root, parent, children, count = c.c_ulong(), c.c_ulong(), c.POINTER(c.c_ulong)(), c.c_uint()
        self.x.XQueryTree(self.display, self.root, c.byref(root), c.byref(parent), c.byref(children), c.byref(count))
        try:
            for index in range(count.value):
                window = children[index]
                if self.title(window).startswith("dng-png-viewer |"):
                    return window
        finally:
            if children:
                self.x.XFree(children)
        return None

    def geometry(self, window):
        root, x, y = c.c_ulong(), c.c_int(), c.c_int()
        width, height, border, depth = c.c_uint(), c.c_uint(), c.c_uint(), c.c_uint()
        ok = self.x.XGetGeometry(self.display, window, c.byref(root), c.byref(x), c.byref(y),
                                c.byref(width), c.byref(height), c.byref(border), c.byref(depth))
        assert ok, "Cannot read viewer geometry"
        return x.value, y.value, width.value, height.value

    def keys(self, window, names):
        self.x.XSetInputFocus(self.display, window, 2, 0)
        for name in names:
            code = self.x.XKeysymToKeycode(self.display, self.x.XStringToKeysym(name.encode()))
            assert code, name
            assert self.xt.XTestFakeKeyEvent(self.display, code, 1, 0)
            assert self.xt.XTestFakeKeyEvent(self.display, code, 0, 0)
        self.x.XSync(self.display, 0)

    def pixels(self, window):
        pointer = self.x.XGetImage(self.display, window, 0, 0, WIDTH, HEIGHT, c.c_ulong(-1), 2)
        assert pointer, "Cannot capture native window"
        try:
            image = pointer.contents
            assert image.bits_per_pixel == 32 and image.bytes_per_line == WIDTH * 4
            assert (image.red_mask, image.green_mask, image.blue_mask) == (0xFF0000, 0xFF00, 0xFF)
            pixels = array.array("I", c.string_at(image.data, image.bytes_per_line * image.height))
            assert pixels.itemsize == 4
            if (image.byte_order == 0) != (sys.byteorder == "little"):
                pixels.byteswap()
            return pixels
        finally:
            self.x.XDestroyImage(pointer)


@contextlib.contextmanager
def server(root):
    executable = shutil.which("Xvfb")
    if not executable:
        raise RuntimeError("Xvfb is required for make test-display")
    read_fd, write_fd = os.pipe()
    with (root / "xvfb.log").open("wb") as log:
        process = subprocess.Popen(
            [executable, "-displayfd", str(write_fd), "-screen", "0", f"{WIDTH}x{HEIGHT}x24",
             "-nolisten", "tcp", "-ac", "-noreset"],
            pass_fds=(write_fd,), stdout=subprocess.DEVNULL, stderr=log,
        )
        os.close(write_fd)
        display = None
        try:
            ready, _, _ = select.select([read_fd], [], [], 20)
            assert ready, "Xvfb did not assign an isolated display"
            number = os.read(read_fd, 64).strip()
            assert number.isdigit(), (root / "xvfb.log").read_text()
            name = ":" + number.decode()
            display = Display(name)
            assert process.poll() is None
            yield display, name
        finally:
            os.close(read_fd)
            if display:
                display.close()
            stop(process)


@contextlib.contextmanager
def viewer(binary, args, root, display_name, log_name):
    environment = os.environ.copy()
    for name in ("WAYLAND_DISPLAY", "SDL_VIDEODRIVER", "SDL_RENDER_DRIVER"):
        environment.pop(name, None)
    environment.update(DISPLAY=display_name, PATH="", LIBGL_ALWAYS_SOFTWARE="1",
                       LP_NUM_THREADS="4", RAYON_NUM_THREADS="4")
    log_path = root / log_name
    with log_path.open("wb") as log:
        process = subprocess.Popen([str(binary), *map(str, args)], env=environment,
                                   stdin=subprocess.DEVNULL, stdout=log, stderr=log)
        try:
            yield process
        except BaseException:
            stop(process)
            print(log_path.read_text(errors="replace"), file=sys.stderr)
            raise
        finally:
            stop(process)


def png_bytes(width, height, depth, color, rows):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, depth, color, 0, 0, 0))
            + chunk(b"gAMA", struct.pack(">I", 100000))
            + chunk(b"IDAT", zlib.compress(b"".join(b"\0" + row for row in rows)))
            + chunk(b"IEND", b""))


def loaded_window(display, process, expected):
    def ready():
        assert process.poll() is None, f"Viewer exited unexpectedly with {process.returncode}"
        window = display.viewer_window()
        return window if window and expected in display.title(window) else None
    return wait_for(ready, f"window showing {expected}")


def wait_title(display, window, expected):
    wait_for(lambda: expected in display.title(window), f"title containing {expected}")


def frame_matches(display, window, expected):
    def ready():
        frame = display.pixels(window)
        return all(frame[y * WIDTH + x] & 0xFFFFFF == rgb for x, y, rgb in expected)
    wait_for(ready, "exact native framebuffer pixels")


def gray_region(shift):
    left, top = (WIDTH - 256) // 2, (HEIGHT - 256) // 2
    return [(left + code % 256, top + code // 256, min(code // (2 ** shift), 255) * 0x010101)
            for code in range(65536)]


def large_sample(x, y):
    return (x * 17 + y * 257) % 65536


def large_probes(left, top):
    return [(x, y, (large_sample(x - left, y - top) // 256) * 0x010101)
            for x in (100, 300, 511, 750, 900) for y in (100, 250, 400, 600, 700)]


def fingerprint(path):
    return hashlib.sha256(path.read_bytes()).hexdigest(), path.stat().st_mtime_ns


def exercise(binary, root, display, display_name):
    gray = root / "all-codes.png"
    gray.write_bytes(png_bytes(256, 256, 16, 0,
                              (struct.pack(">256H", *range(y * 256, (y + 1) * 256)) for y in range(256))))
    normal = root / "normal.png"
    normal.write_bytes(png_bytes(256, 256, 8, 6,
                                (bytes(v for x in range(256) for v in (x, y, (x + y) % 256, (0, 64, 128, 255)[x % 4]))
                                 for y in range(256))))
    large = root / "large.png"
    large.write_bytes(png_bytes(1300, 1000, 16, 0,
                               (struct.pack(">1300H", *(large_sample(x, y) for x in range(1300))) for y in range(1000))))
    broken = root / "broken.jpg"
    broken.write_bytes(b"deliberately invalid synthetic image")
    recovery = root / "recovery.png"
    one_pixel = png_bytes(1, 1, 8, 2, [bytes([19, 57, 121])])
    recovery.write_bytes(one_pixel)
    originals = {path: fingerprint(path) for path in (gray, normal, large, broken, recovery)}

    with viewer(binary, [gray, normal, large, broken, recovery], root, display_name, "viewer.log") as process:
        window = loaded_window(display, process, "FIT | Gray16 256x256 | 16 code bits | window 4-12")
        assert display.geometry(window) == (0, 0, WIDTH, HEIGHT)
        display.keys(window, ["Left"])
        wait_title(display, window, "| 1/5 |")
        shift = 4
        display.keys(window, ["z"])
        wait_title(display, window, "| 1:1 |")
        frame_matches(display, window, gray_region(shift))
        for target in range(9):
            display.keys(window, ["x"] + (["Up"] * (shift - target) if target < shift else ["Down"] * (target - shift)) + ["z"])
            wait_title(display, window, f"1:1 | Gray16 256x256 | 16 code bits | window {target}-{target + 8}")
            frame_matches(display, window, gray_region(target))
            shift = target
        print("PASS: fullscreen, first/end navigation, all 65,536 samples in all nine native DR windows", flush=True)

        display.keys(window, ["x", "Right"])
        wait_title(display, window, "FIT | standard 256x256")
        display.keys(window, ["z"])
        wait_title(display, window, "1:1 | standard 256x256")
        left, top = (WIDTH - 256) // 2, (HEIGHT - 256) // 2
        expected = []
        for y in range(256):
            for x in range(256):
                alpha = (0, 64, 128, 255)[x % 4]
                rgb = [(value * alpha + 127) // 255 for value in (x, y, (x + y) % 256)]
                expected.append((left + x, top + y, rgb[0] << 16 | rgb[1] << 8 | rgb[2]))
        frame_matches(display, window, expected)
        display.keys(window, ["x", "Up", "Down", "z"])
        wait_title(display, window, "1:1 | standard 256x256")
        frame_matches(display, window, expected)
        print("PASS: ordinary RGBA fallback, exact alpha composition and disabled DR actions", flush=True)

        display.keys(window, ["x", "Right"])
        wait_title(display, window, "FIT | Gray16 1300x1000")
        display.keys(window, ["Down"] * 4 + ["z"])
        wait_title(display, window, "1:1 | Gray16 1300x1000 | 16 code bits | window 8-16")
        frame_matches(display, window, large_probes(-138, -116))
        for keys, origin in [
            (["Left"], (-202, -116)), (["Up"], (-202, -180)),
            (["Right"], (-138, -180)), (["Down"], (-138, -116)),
            (["Left"] * 20 + ["Up"] * 20, (-276, -232)),
            (["Right"] * 20 + ["Down"] * 20, (0, 0)),
            (["z"], (-138, -116)),
        ]:
            display.keys(window, keys)
            frame_matches(display, window, large_probes(*origin))
            assert "window 8-16" in display.title(window)
            assert "| 3/5 |" in display.title(window)
        display.keys(window, ["x"])
        wait_title(display, window, "FIT | Gray16 1300x1000 | 16 code bits | window 8-16")
        print("PASS: Z/X, all pan directions, edge clamps and preserved DR selection", flush=True)

        display.keys(window, ["Right"])
        wait_title(display, window, "Cannot load image:")
        display.keys(window, ["Right"])
        wait_title(display, window, "FIT | standard 1x1")
        display.keys(window, ["Right"])
        wait_title(display, window, "| 5/5 |")
        frame_matches(display, window, [(WIDTH // 2, HEIGHT // 2, 0x133979)])
        display.keys(window, ["Escape"])
        assert process.wait(timeout=5) == 1, "Decode failures must produce a nonzero final exit"
        assert "Cannot load image" in (root / "viewer.log").read_text()
        print("PASS: visible decode error, recovery, last-image boundary and Escape", flush=True)

    for path, before in originals.items():
        assert fingerprint(path) == before, f"Input changed: {path}"

    many = root / "many"
    many.mkdir()
    argument_limit = os.sysconf("SC_ARG_MAX")
    argument_bytes, count = 0, 0
    while argument_bytes <= argument_limit + 4096:
        path = many / (f"{count:06}-" + "x" * 190 + ".png")
        path.write_bytes(one_pixel)
        argument_bytes += len(os.fsencode(path)) + 1
        count += 1
    with viewer(binary, ["-shuffle", "-dir", many], root, display_name, "many.log") as process:
        window = loaded_window(display, process, "FIT | standard 1x1")
        assert f"| 1/{count} |" in display.title(window)
        frame_matches(display, window, [(WIDTH // 2, HEIGHT // 2, 0x133979)])
        display.keys(window, ["Escape"])
        assert process.wait(timeout=5) == 0
    print(f"PASS: -dir/-shuffle loads {count} files, {argument_bytes} filename bytes > ARG_MAX={argument_limit}", flush=True)

    slow = root / "loading.png"
    row = struct.pack(">8192H", *([65535] * 8192))
    slow.write_bytes(png_bytes(8192, 4096, 16, 0, [row] * 4096))
    with viewer(binary, [slow], root, display_name, "loading.log") as process:
        window = loaded_window(display, process, "Loading...")
        start = time.monotonic()
        display.keys(window, ["Escape"])
        assert process.wait(timeout=3) == 0
        assert time.monotonic() - start < 3
    print("PASS: Escape remains responsive during a 64-MiB image decode; PATH was empty throughout", flush=True)
    unexpected_errors = [error for error in display.errors if error[0] != 3]
    assert not unexpected_errors, f"Unexpected X11 errors: {unexpected_errors}"


def main():
    if len(sys.argv) != 2:
        raise SystemExit("Usage: python3 tests/display.py /path/to/dng-png-viewer")
    binary = Path(sys.argv[1]).resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="dng-png-viewer-test-") as temporary:
        root = Path(temporary)
        with server(root) as (display, display_name):
            exercise(binary, root, display, display_name)
    print("All native display checks passed; isolated server/processes/fixtures removed.", flush=True)


if __name__ == "__main__":
    main()
