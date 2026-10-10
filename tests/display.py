#!/usr/bin/env python3
"""Native X11 integration tests; only an isolated Xvfb and synthetic files are used."""

import array
import contextlib
import ctypes as c
import ctypes.util
import fcntl
import hashlib
import json
import os
from pathlib import Path
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
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

    def chord(self, window, names):
        self.x.XSetInputFocus(self.display, window, 2, 0)
        codes = [self.x.XKeysymToKeycode(self.display, self.x.XStringToKeysym(name.encode())) for name in names]
        assert all(codes)
        for code in codes:
            assert self.xt.XTestFakeKeyEvent(self.display, code, 1, 0)
        for code in reversed(codes):
            assert self.xt.XTestFakeKeyEvent(self.display, code, 0, 0)
        self.x.XSync(self.display, 0)


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


def gray_region(shift, width=256, height=256):
    left, top = (WIDTH - width) // 2, (HEIGHT - height) // 2
    return [(left + code % width, top + code // width, int(min(code / (2 ** shift), 255)) * 0x010101)
            for code in range(width * height)]


def large_sample(x, y):
    return (x * 17 + y * 257) % 65536


def large_probes(left, top):
    return [(x, y, (large_sample(x - left, y - top) // 256) * 0x010101)
            for x in (100, 300, 511, 750, 900) for y in (100, 250, 400, 600, 700)]


def fingerprint(path):
    return hashlib.sha256(path.read_bytes()).hexdigest(), path.stat().st_mtime_ns


def attach_pty():
    os.setsid()
    fcntl.ioctl(0, termios.TIOCSCTTY, 0)


def reject_unsafe_console(binary, image):
    if not sys.platform.startswith("freebsd"):
        return
    environment = os.environ.copy()
    environment.update(SDL_VIDEODRIVER="KMSDRM", DISPLAY="", WAYLAND_DISPLAY="", PATH="")
    result = subprocess.run([str(binary), str(image)], env=environment,
                            start_new_session=True, capture_output=True, timeout=5)
    assert result.returncode != 0
    assert b"active physical console" in result.stderr
    assert b"Native SDL2" not in result.stderr

    master, slave = os.openpty()

    try:
        result = subprocess.run([str(binary), str(image)], env=environment,
                                stdin=slave, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                preexec_fn=attach_pty, timeout=5)
        assert result.returncode != 0
        assert b"physical FreeBSD VT" in result.stderr
        assert b"Native SDL2" not in result.stderr
    finally:
        os.close(slave)
        os.close(master)
    print("PASS: FreeBSD KMS rejects missing/PTY consoles before SDL initialization", flush=True)


def console_input_pipeline(test_binary, image, root, display, display_name):
    if not sys.platform.startswith("freebsd") or test_binary is None:
        return
    environment = os.environ.copy()
    environment.update(SDL_VIDEODRIVER="x11", DISPLAY=display_name, PATH="",
                       RAYON_NUM_THREADS="4", LIBGL_ALWAYS_SOFTWARE="1", LP_NUM_THREADS="4",
                       DNG_VIEWER_TEST_IMAGE=str(image))
    for exit_method in ("Escape", "q", "Q", "Ctrl+C", signal.SIGINT, signal.SIGTERM, signal.SIGUSR1):
        master, slave = os.openpty()
        log_path = root / "console-input.log"
        process = None
        try:
            with log_path.open("wb") as log:
                process = subprocess.Popen(
                    [str(test_binary), "--exact", "backend::tests::console_input_pipeline",
                     "--ignored", "--nocapture"], env=environment, stdin=slave,
                    stdout=log, stderr=log, preexec_fn=attach_pty,
                )
                window = loaded_window(display, process, "FIT | Gray16 256x256 | 16 code bits | window 8-16")
                if exit_method == "Escape":
                    os.write(master, b"\x1b")
                    time.sleep(0.01)
                    os.write(master, b"[AZ")
                    wait_title(display, window, "1:1 | Gray16 256x256 | 16 code bits | window 7.8-15.8")
                    frame_matches(display, window, gray_region(7.8))
                    os.write(master, b"X\x1b[Bz")
                    wait_title(display, window, "1:1 | Gray16 256x256 | 16 code bits | window 8-16")
                    frame_matches(display, window, gray_region(8))
                    os.write(master, b"\x1b")
                elif exit_method in ("q", "Q", "Ctrl+C"):
                    os.write(master, b"\x03" if exit_method == "Ctrl+C" else exit_method.encode())
                else:
                    process.send_signal(exit_method)
                assert process.wait(timeout=5) == 0, log_path.read_text()
        except BaseException:
            if process:
                stop(process)
            print(log_path.read_text(errors="replace"), file=sys.stderr)
            raise
        finally:
            if process:
                stop(process)
            os.close(slave)
            os.close(master)
    print("PASS: real PTY arrows/Z/X drive native pixels; Escape/Q/Ctrl+C/INT/TERM/VT-release restore input", flush=True)


def clean_exits(binary, image, root, display, display_name, expected):
    for exit_method in ("q", "Q", "Ctrl+C", signal.SIGINT, signal.SIGTERM):
        with viewer(binary, [image], root, display_name, "clean-exit.log") as process:
            window = loaded_window(display, process, expected)
            if exit_method == "Ctrl+C":
                display.chord(window, ["Control_L", "c"])
            elif exit_method == "Q":
                display.chord(window, ["Shift_L", "q"])
            elif exit_method == "q":
                display.keys(window, ["q"])
            else:
                process.send_signal(exit_method)
            assert process.wait(timeout=3) == 0, f"Unclean exit: {exit_method}"
    print(f"PASS: Q/Ctrl+C keyboard shortcuts and INT/TERM signals cleanly exit from {expected}", flush=True)


def preserved_windows(binary, root, display, display_name, gray, normal, broken):
    matching, after_normal, after_error = [root / f"{name}.png"
                                         for name in ("matching", "after-normal", "after-error")]
    pixels = gray.read_bytes()
    for path in (matching, after_normal, after_error):
        path.write_bytes(pixels)
    resized = root / "same-range-different-size.png"
    resized.write_bytes(png_bytes(128, 512, 16, 0,
                                 (struct.pack(">128H", *range(y * 128, (y + 1) * 128))
                                  for y in range(512))))
    narrow = root / "twelve-bit.png"
    narrow.write_bytes(png_bytes(2, 1, 16, 0, [struct.pack(">2H", 0, 4095)]))
    paths = [gray, matching, resized, narrow, normal, after_normal, broken, after_error]
    for preload in (0, 3):
        with viewer(binary, ["-preload", preload, *paths], root, display_name,
                    f"preserved-window-{preload}.log") as process:
            window = loaded_window(display, process, "FIT | Gray16 256x256 | 16 code bits | window 8-16")
            display.keys(window, ["Up"] * 15)
            wait_title(display, window, "window 5-13")
            display.keys(window, ["Right", "Right", "z"])
            wait_title(display, window, "1:1 | Gray16 128x512 | 16 code bits | window 5-13")
            frame_matches(display, window, gray_region(5, 128, 512))
            display.keys(window, ["x"] + ["Up"] * 3 + ["Left", "Left", "z"])
            wait_title(display, window, "1:1 | Gray16 256x256 | 16 code bits | window 4.4-12.4")
            frame_matches(display, window, gray_region(4.4))
            display.keys(window, ["x", "Right"])
            wait_title(display, window, "FIT | Gray16 256x256 | 16 code bits | window 4.4-12.4")
            display.keys(window, ["Up"] * 12 + ["Right", "Right"])
            wait_title(display, window, "FIT | Gray16 2x1 | 12 code bits | window 4-12")
            display.keys(window, ["Up"] * 3 + ["Left"])
            wait_title(display, window, "FIT | Gray16 128x512 | 16 code bits | window 8-16")
            display.keys(window, ["Right"])
            wait_title(display, window, "FIT | Gray16 2x1 | 12 code bits | window 4-12")
            display.keys(window, ["Up"] * 3 + ["Right"])
            wait_title(display, window, "FIT | standard 256x256")
            display.keys(window, ["Left"])
            wait_title(display, window, "FIT | Gray16 2x1 | 12 code bits | window 4-12")
            display.keys(window, ["Right", "Right"])
            wait_title(display, window, "FIT | Gray16 256x256 | 16 code bits | window 8-16")
            display.keys(window, ["Up"] * 15 + ["Right"])
            wait_title(display, window, "Cannot load image:")
            display.keys(window, ["Right"])
            wait_title(display, window, "FIT | Gray16 256x256 | 16 code bits | window 8-16")
            display.keys(window, ["Escape"])
            assert process.wait(timeout=5) == 1
    print("PASS: DR persists through cached/cold and rapid navigation; different ranges, color and errors reset it",
          flush=True)


def downsampling(binary, root, display, display_name):
    width, height = WIDTH * 4, HEIGHT * 4
    samples = [65535] + [n * 4096 for n in range(1, 16)]
    encoded = png_bytes(width, height, 16, 0,
                        (struct.pack(">4H", *samples[(y % 4) * 4:(y % 4 + 1) * 4]) * WIDTH
                         for y in range(height)))
    paths = [root / "downsample.png", root / "downsample-copy.png"]
    for path in paths:
        path.write_bytes(encoded)
    originals = {path: fingerprint(path) for path in paths}
    probes = [(x, y) for x in range(100, 104) for y in range(100, 104)]
    for args, filtered in [
        ([], range(16)),
        (["-downsample", "area"], range(16)),
        (["--downsample=full"], range(16)),
        (["--downsample=bilinear"], (5, 6, 9, 10)),
        (["-downsample", "random"], None),
        (["--downsample=middle"], (10,)),
        (["-downsample", "NE"], (3,)),
        (["-downsample", "NW"], (0,)),
        (["-downsample", "SE"], (15,)),
        (["-downsample", "SW"], (12,)),
    ]:
        with viewer(binary, [*args, *paths], root, display_name, "downsampling.log") as process:
            window = loaded_window(display, process, f"FIT | Gray16 {width}x{height}")
            if filtered is None:
                palette = {min(value // 256, 255) * 0x010101: i for i, value in enumerate(samples)}
                assert len(palette) == 16

                def random_picks():
                    frame = display.pixels(window)
                    colors = [frame[y * WIDTH + x] & 0xFFFFFF for x, y in probes]
                    return [palette[color] for color in colors] if all(color in palette for color in colors) else None

                picked = wait_for(random_picks, "one actual source sample per random output pixel")
            for shift in (8, 7.8):
                if shift != 8:
                    display.keys(window, ["Up"])
                    wait_title(display, window, "window 7.8-15.8")
                values = [min(int(value / (2 ** shift)), 255) for value in samples]
                if filtered is None:
                    expected = [(x, y, values[i] * 0x010101) for (x, y), i in zip(probes, picked)]
                else:
                    mean = (sum(values[i] for i in filtered) + len(filtered) // 2) // len(filtered)
                    expected = [(x, y, mean * 0x010101) for x, y in probes]
                frame_matches(display, window, expected)
            display.keys(window, ["z"])
            wait_title(display, window, "1:1 |")
            frame_matches(display, window,
                          [(x, y, values[(y % 4) * 4 + x % 4] * 0x010101) for x, y in probes])
            display.keys(window, ["x", "Right"])
            wait_title(display, window, "| 2/2 |")
            wait_title(display, window, "window 7.8-15.8")
            frame_matches(display, window, expected)
            display.keys(window, ["Left"])
            wait_title(display, window, "| 1/2 |")
            wait_title(display, window, "window 7.8-15.8")
            frame_matches(display, window, expected)
            display.keys(window, ["Escape"])
            assert process.wait(timeout=5) == 0
    for path, before in originals.items():
        assert fingerprint(path) == before, f"Input changed: {path}"
    print("PASS: all downsample filters and full alias; stable random pixels, fractional DR, 1:1 and navigation",
          flush=True)


def exercise(binary, test_binary, root, display, display_name):
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
    reject_unsafe_console(binary, gray)
    console_input_pipeline(test_binary, gray, root, display, display_name)
    clean_exits(binary, gray, root, display, display_name, "FIT | Gray16")

    with viewer(binary, [gray, normal, large, broken, recovery], root, display_name, "viewer.log") as process:
        window = loaded_window(display, process, "FIT | Gray16 256x256 | 16 code bits | window 8-16")
        assert display.geometry(window) == (0, 0, WIDTH, HEIGHT)
        display.keys(window, ["Left"])
        wait_title(display, window, "| 1/5 |")
        shift = 8
        display.keys(window, ["z"])
        wait_title(display, window, "| 1:1 |")
        frame_matches(display, window, gray_region(shift))
        for fifth in range(41):
            target = fifth / 5
            steps = round(abs(shift - target) / 0.2)
            display.keys(window, ["x"] + (["Up"] if target < shift else ["Down"]) * steps + ["z"])
            wait_title(display, window, f"1:1 | Gray16 256x256 | 16 code bits | window {target:g}-{target + 8:g}")
            frame_matches(display, window, gray_region(target))
            shift = target
        print("PASS: fullscreen, first/end navigation, all 65,536 samples in all 41 fifth-bit native DR windows", flush=True)

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
        wait_title(display, window, "FIT | Gray16 1300x1000 | 16 code bits | window 8-16")
        display.keys(window, ["z"])
        wait_title(display, window, "1:1 | Gray16 1300x1000 | 16 code bits | window 8-16")
        frame_matches(display, window, large_probes(-138, -116))
        for keys, origin in [
            (["Left"], (-74, -116)), (["Up"], (-74, -52)),
            (["Right"], (-138, -52)), (["Down"], (-138, -116)),
            (["Left"] * 20 + ["Up"] * 20, (0, 0)),
            (["Right"] * 20 + ["Down"] * 20, (-276, -232)),
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
        frame_matches(display, window, [(WIDTH // 2, 2, 0x061328)])
        frame_matches(display, window, [(WIDTH // 2, 2, 0x133979)])
        display.keys(window, ["Escape"])
        assert process.wait(timeout=5) == 1, "Decode failures must produce a nonzero final exit"
        assert "Cannot load image" in (root / "viewer.log").read_text()
        print("PASS: visible decode error, recovery, HUD expiry, last-image boundary and Escape", flush=True)

    for preload in (0, 1, 3):
        with viewer(binary, ["-preload", preload, "-brightness-step", "0.5", gray, normal, recovery],
                    root, display_name, f"preload-{preload}.log") as process:
            window = loaded_window(display, process, "window 8-16")
            display.keys(window, ["Up", "z"])
            wait_title(display, window, "1:1 | Gray16 256x256 | 16 code bits | window 7.5-15.5")
            frame_matches(display, window, gray_region(7.5))
            display.keys(window, ["x", "Right", "Right", "z"])
            wait_title(display, window, "1:1 | standard 1x1")
            frame_matches(display, window, [((WIDTH - 1) // 2, (HEIGHT - 1) // 2, 0x133979)])
            display.keys(window, ["x", "Left", "Left"])
            wait_title(display, window, "FIT | Gray16 256x256 | 16 code bits | window 8-16")
            display.keys(window, ["Up", "z"])
            wait_title(display, window, "window 7.5-15.5")
            frame_matches(display, window, gray_region(7.5))
            display.keys(window, ["Escape"])
            assert process.wait(timeout=5) == 0
    print("PASS: preload 0/1/3, rapid forward/back navigation and configurable half-bit controls", flush=True)

    with viewer(binary, [gray, broken], root, display_name, "unvisited-error.log") as process:
        window = loaded_window(display, process, "FIT | Gray16")
        display.keys(window, ["Up", "z"])
        wait_title(display, window, "window 7.8-15.8")
        frame_matches(display, window, gray_region(7.8))
        display.keys(window, ["Escape"])
        assert process.wait(timeout=5) == 0, "An unvisited preload failure must not fail the current image"

    preserved_windows(binary, root, display, display_name, gray, normal, broken)
    downsampling(binary, root, display, display_name)

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
    clean_exits(binary, slow, root, display, display_name, "Loading...")
    with viewer(binary, [gray, slow], root, display_name, "preloading-exit.log") as process:
        window = loaded_window(display, process, "FIT | Gray16 256x256")
        start = time.monotonic()
        display.keys(window, ["Up", "z"])
        wait_title(display, window, "window 7.8-15.8")
        frame_matches(display, window, gray_region(7.8))
        display.keys(window, ["Escape"])
        assert process.wait(timeout=3) == 0
        assert time.monotonic() - start < 3
    print("PASS: brightness, zoom and Escape remain responsive while a large neighbor preloads", flush=True)
    # SDL can replace its startup window while creating the renderer.
    unexpected_errors = [error for error in display.errors if error[0] != 3]
    assert not unexpected_errors, f"Unexpected X11 errors: {unexpected_errors}"


def main():
    if len(sys.argv) not in (2, 3):
        raise SystemExit("Usage: python3 tests/display.py /path/to/dng-png-viewer [cargo-test-artifacts.jsonl]")
    binary = Path(sys.argv[1]).resolve(strict=True)
    test_binary = None
    if len(sys.argv) == 3:
        records = [json.loads(line) for line in Path(sys.argv[2]).read_text().splitlines()]
        matches = [record["executable"] for record in records
                   if record.get("reason") == "compiler-artifact"
                   and record.get("target", {}).get("name") == "dng_png_viewer"
                   and record.get("profile", {}).get("test") and record.get("executable")]
        assert len(matches) == 1, "Expected exactly one viewer library test executable"
        test_binary = Path(matches[0]).resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="dng-png-viewer-test-") as temporary:
        root = Path(temporary)
        with server(root) as (display, display_name):
            exercise(binary, test_binary, root, display, display_name)
    print("All native display checks passed; isolated server/processes/fixtures removed.", flush=True)


if __name__ == "__main__":
    main()
