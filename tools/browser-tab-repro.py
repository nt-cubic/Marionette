"""Reproduce/diagnose the browser-tab overlay bug against a live Marionette.

Launches the exe with a remote-debugging port on the WebView2s, drives the
APP webview over CDP (Runtime.evaluate -> __TAURI_INTERNALS__.invoke), then:

  1. opens a browser tab exactly like the palette does;
  2. reads the DOM host rect (.browser-page__host) in CSS px;
  3. enumerates the main window's child HWNDs and reports every WRY_WEBVIEW
     container's client rect (in px) — where the native surface ACTUALLY is;
  4. checks app-webview responsiveness with a timed invoke.

Usage: python tools/browser-tab-repro.py <Marionette.exe> [--keep]
"""

import base64
import hashlib
import json
import os
import socket
import struct
import subprocess
import sys
import time
import urllib.request
import uuid
import ctypes
import ctypes.wintypes as wt

DEBUG_PORT = 9333


# ── minimal RFC6455 websocket client (CDP needs one; stdlib has none) ──────

class WS:
    def __init__(self, host, port, path):
        self.sock = socket.create_connection((host, port), timeout=20)
        key = base64.b64encode(os.urandom(16)).decode()
        request = (
            f"GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\n"
            "Upgrade: websocket\r\nConnection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        )
        self.sock.sendall(request.encode())
        response = b""
        while b"\r\n\r\n" not in response:
            response += self.sock.recv(4096)
        assert b"101" in response.split(b"\r\n")[0], response[:200]
        self.buffer = b""

    def send_text(self, text):
        payload = text.encode()
        header = bytearray([0x81])
        length = len(payload)
        mask = os.urandom(4)
        if length < 126:
            header.append(0x80 | length)
        elif length < 65536:
            header.append(0x80 | 126)
            header += struct.pack(">H", length)
        else:
            header.append(0x80 | 127)
            header += struct.pack(">Q", length)
        header += mask
        masked = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
        self.sock.sendall(bytes(header) + masked)

    def _read(self, count):
        while len(self.buffer) < count:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise EOFError
            self.buffer += chunk
        out, self.buffer = self.buffer[:count], self.buffer[count:]
        return out

    def recv_text(self):
        first = self._read(1)[0]
        assert first & 0x0F == 1, f"non-text frame: {first:#x}"
        second = self._read(1)[0]
        length = second & 0x7F
        if length == 126:
            length = struct.unpack(">H", self._read(2))[0]
        elif length == 127:
            length = struct.unpack(">Q", self._read(8))[0]
        return self._read(length).decode("utf-8", "replace")


class Cdp:
    def __init__(self, url):
        scheme, rest = url.split("://", 1)
        hostport, path = rest.split("/", 1)
        host, port = hostport.split(":")
        self.ws = WS(host, int(port), "/" + path)
        self.next_id = 1

    def call(self, method, **params):
        call_id = self.next_id
        self.next_id += 1
        self.ws.send_text(json.dumps({"id": call_id, "method": method, "params": params}))
        while True:
            message = json.loads(self.ws.recv_text())
            if message.get("id") == call_id:
                if "error" in message:
                    raise RuntimeError(f"{method}: {message['error']}")
                return message.get("result", {})

    def evaluate(self, expression, await_promise=False):
        result = self.call(
            "Runtime.evaluate",
            expression=expression,
            returnByValue=True,
            awaitPromise=await_promise,
        )
        if result.get("exceptionDetails"):
            raise RuntimeError(result["exceptionDetails"].get("text"))
        return result.get("result", {}).get("value")


# ── HWND inspection ─────────────────────────────────────────────────────────

user32 = ctypes.WinDLL("user32", use_last_error=True)


def child_windows(needle_class_prefix):
    found = []

    @ctypes.WINFUNCTYPE(ctypes.c_bool, wt.HWND, wt.LPARAM)
    def callback(hwnd, lparam):
        buffer = ctypes.create_unicode_buffer(64)
        user32.GetClassNameW(hwnd, buffer, 64)
        if buffer.value.startswith(needle_class_prefix) and user32.IsWindowVisible(hwnd):
            rect = wt.RECT()
            user32.GetWindowRect(hwnd, ctypes.byref(rect))
            parent = user32.GetAncestor(hwnd, 1)  # GA_PARENT
            parent_rect = wt.RECT()
            user32.GetWindowRect(parent, ctypes.byref(parent_rect))
            # child coords relative to the parent's client area
            point = wt.POINT(rect.left, rect.top)
            user32.ScreenToClient(parent, ctypes.byref(point))
            title = ctypes.create_unicode_buffer(128)
            user32.GetWindowTextW(hwnd, title, 128)
            found.append(
                {
                    "hwnd": hwnd,
                    "class": buffer.value,
                    "x": point.x,
                    "y": point.y,
                    "w": rect.right - rect.left,
                    "h": rect.bottom - rect.top,
                    "title": title.value,
                }
            )
        return True

    user32.EnumWindows(callback, 0)
    return found


def marionette_hwnd():
    best = []

    @ctypes.WINFUNCTYPE(ctypes.c_bool, wt.HWND, wt.LPARAM)
    def callback(hwnd, _):
        if not user32.IsWindowVisible(hwnd):
            return True
        buffer = ctypes.create_unicode_buffer(128)
        user32.GetWindowTextW(hwnd, buffer, 128)
        if buffer.value == "Marionette":
            best.append(hwnd)
        return True

    user32.EnumWindows(callback, 0)
    return best[0] if best else None


def children_of(hwnd):
    out = []

    @ctypes.WINFUNCTYPE(ctypes.c_bool, wt.HWND, wt.LPARAM)
    def callback(child, _):
        buffer = ctypes.create_unicode_buffer(64)
        user32.GetClassNameW(child, buffer, 64)
        rect = wt.RECT()
        user32.GetWindowRect(child, ctypes.byref(rect))
        out.append({"class": buffer.value, "x": rect.left, "y": rect.top, "w": rect.right - rect.left, "h": rect.bottom - rect.top})
        return True

    user32.EnumChildWindows(hwnd, callback, 0)
    return out


def main():
    exe = sys.argv[1]
    env = dict(os.environ)
    env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = f"--remote-debugging-port={DEBUG_PORT}"
    profile = os.path.join(os.environ["TEMP"], "marionette-browser-repro")
    env["USERPROFILE"] = profile
    env["WEBVIEW2_USER_DATA_FOLDER"] = os.path.join(profile, "webview2")
    subprocess.Popen([exe], env=env, cwd=exe.rsplit("\\", 1)[0])
    print("launched; waiting for the debugging port…")

    targets = None
    for _ in range(60):
        time.sleep(1)
        try:
            targets = json.load(urllib.request.urlopen(f"http://127.0.0.1:{DEBUG_PORT}/json", timeout=2))
            if targets:
                break
        except Exception:
            continue
    assert targets, "debug port never came up"
    page = next((t for t in targets if t["type"] == "page"), None)
    assert page, f"no page target: {[t['type'] for t in targets]}"
    # Prefer the app's own page (title Marionette) over any browser tab page.
    for candidate in targets:
        if candidate["type"] == "page" and "Marionette" in (candidate.get("title") or ""):
            page = candidate
            break
    print(f"app target: {page['title']!r}")

    cdp = Cdp(page["webSocketDebuggerUrl"])
    time.sleep(3)

    started = time.time()
    cdp.evaluate(
        "window.__TAURI_INTERNALS__.invoke('browser_open_tab', { url: 'https://example.com' })",
        await_promise=True,
    )
    print(f"browser_open_tab invoke returned in {time.time() - started:.1f}s")

    time.sleep(4)  # let the tabs event + effects settle

    host_rect = cdp.evaluate(
        "JSON.stringify((document.querySelector('.browser-page__host') || {getBoundingClientRect: () => null}).getBoundingClientRect())"
    )
    print("DOM host rect (CSS px):", host_rect)

    responsive = cdp.evaluate(
        "Promise.race([window.__TAURI_INTERNALS__.invoke('browser_list_tabs').then(v => JSON.stringify('ok:' + v.length)), new Promise(r => setTimeout(() => r('TIMEOUT'), 5000))])",
        await_promise=True,
    )
    print("app invoke responsiveness:", responsive)

    hwnd = marionette_hwnd()
    print("main window:", hex(hwnd) if hwnd else None)
    if hwnd:
        rect = wt.RECT()
        user32.GetWindowRect(hwnd, ctypes.byref(rect))
        print(f"main window rect: {rect.left},{rect.top} {rect.right - rect.left}x{rect.bottom - rect.top}")
        for child in children_of(hwnd):
            print("  child:", child)

    if "--keep" not in sys.argv:
        subprocess.run(["taskkill", "/IM", "Marionette.exe", "/F"], capture_output=True)
        import shutil

        shutil.rmtree(profile, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
