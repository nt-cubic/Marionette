"""Capture a top-level window by (partial) title via PrintWindow.

PW_RENDERFULLCONTENT (2) is what renders WebView2 content — a plain BitBlt
from the screen would need the window in front (see tools/README.md).

Usage: python tools/capture-window.py <title-substring> <out.png> [wait-seconds]
"""

import ctypes
import ctypes.wintypes as wt
import sys
import time

user32 = ctypes.WinDLL("user32", use_last_error=True)
gdi32 = ctypes.WinDLL("gdi32", use_last_error=True)

PW_RENDERFULLCONTENT = 2


def find_window(needle: str):
    matches = []

    @ctypes.WINFUNCTYPE(ctypes.c_bool, wt.HWND, wt.LPARAM)
    def callback(hwnd, _lparam):
        length = user32.GetWindowTextLengthW(hwnd)
        if length <= 0:
            return True
        buffer = ctypes.create_unicode_buffer(length + 1)
        user32.GetWindowTextW(hwnd, buffer, length + 1)
        if needle.lower() in buffer.value.lower() and user32.IsWindowVisible(hwnd):
            matches.append((hwnd, buffer.value))
        return True

    user32.EnumWindows(callback, 0)
    return matches


def capture(hwnd, path):
    rect = wt.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(rect))
    width, height = rect.right - rect.left, rect.bottom - rect.top
    hdc_window = user32.GetWindowDC(hwnd)
    hdc_mem = gdi32.CreateCompatibleDC(hdc_window)
    bitmap = gdi32.CreateCompatibleBitmap(hdc_window, width, height)
    gdi32.SelectObject(hdc_mem, bitmap)
    result = user32.PrintWindow(hwnd, hdc_mem, PW_RENDERFULLCONTENT)
    # Save with GDI+ (WIC-free path available on every Windows).
    gdiplus = ctypes.WinDLL("gdiplus")
    class GdiplusStartupInput(ctypes.Structure):
        _fields_ = [
            ("GdiplusVersion", ctypes.c_uint32),
            ("DebugEventCallback", ctypes.c_void_p),
            ("SuppressBackgroundThread", ctypes.c_int),
            ("SuppressExternalCodecs", ctypes.c_int),
        ]
    startup = GdiplusStartupInput()
    startup.GdiplusVersion = 1
    token = ctypes.c_ulonglong()
    gdiplus.GdiplusStartup(ctypes.byref(token), ctypes.byref(startup), None)

    class GUID(ctypes.Structure):
        _fields_ = [
            ("Data1", ctypes.c_uint32),
            ("Data2", ctypes.c_uint16),
            ("Data3", ctypes.c_uint16),
            ("Data4", ctypes.c_ubyte * 8),
        ]

    from ctypes import c_void_p, c_int, c_uint, POINTER

    encoder_png = GUID(
        0x557CF406, 0x1A04, 0x11D3, (ctypes.c_ubyte * 8)(0x9A, 0x73, 0x00, 0x00, 0xF8, 0x1E, 0xF3, 0x2E)
    )
    status = ctypes.c_int()
    # GetHBITMAP-free save: wrap the HBITMAP into a GpBitmap.
    bitmap_ptr = c_void_p()
    gdiplus.GdipCreateBitmapFromHBITMAP(bitmap, None, ctypes.byref(bitmap_ptr))
    gdiplus.GdipSaveImageToFile(bitmap_ptr, ctypes.c_wchar_p(path), ctypes.byref(encoder_png), None)
    if status.value != 0:
        print(f"GdipSaveImageToFile status {status.value:#x}")
    gdiplus.GdipDisposeImage(bitmap_ptr)
    gdiplus.GdiplusShutdown(ctypes.byref(token))
    gdi32.DeleteObject(bitmap)
    gdi32.DeleteDC(hdc_mem)
    user32.ReleaseDC(hwnd, hdc_window)
    return bool(result)


def main():
    needle, path = sys.argv[1], sys.argv[2]
    wait = float(sys.argv[3]) if len(sys.argv) > 3 else 0.0
    deadline = time.time() + max(wait, 20)
    matches = []
    while time.time() < deadline:
        matches = [entry for entry in find_window(needle) if "Marionette" in entry[1] or needle.lower() in entry[1].lower()]
        if matches:
            break
        time.sleep(1)
    if not matches:
        print(f"NO WINDOW matching {needle!r}")
        return 1
    hwnd, title = max(matches, key=lambda entry: entry[0])
    # Give WebView2 a moment to paint after the window exists.
    time.sleep(3)
    ok = capture(hwnd, path)
    print(f"{'CAPTURED' if ok else 'PrintWindow FAILED'}: {title!r} -> {path}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
