"""Self-check: the Computer Use helper protocol of a built Marionette.exe.

Runs `Marionette.exe --marionette-computer-helper` with piped stdio and:
  1. expects a Ready frame with the matching protocol version;
  2. sends a request with an UNKNOWN op -> expects an error reply (closed set);
  3. sends `configure` with a nonexistent driver path -> expects the
     driver-missing error (proves dispatch reaches the helper logic);
  4. closes stdin -> expects the process to exit by itself.

Usage: python tools/probe-computer-helper.py <path-to-Marionette.exe>
"""

import json
import struct
import subprocess
import sys
import time


def read_frame(stream, timeout=15.0):
    deadline = time.time() + timeout
    header = stream.read(4)
    if len(header) != 4:
        raise AssertionError(f"no frame header (got {len(header)} bytes)")
    (length,) = struct.unpack("<I", header)
    assert 0 < length <= 16 * 1024 * 1024, f"frame length out of range: {length}"
    payload = b""
    while len(payload) < length and time.time() < deadline:
        chunk = stream.read(length - len(payload))
        if chunk:
            payload += chunk
    assert len(payload) == length, "truncated frame"
    return json.loads(payload.decode("utf-8"))


def write_frame(stream, frame):
    payload = json.dumps(frame).encode("utf-8")
    stream.write(struct.pack("<I", len(payload)))
    stream.write(payload)
    stream.flush()


def main():
    exe = sys.argv[1]
    proc = subprocess.Popen(
        [exe, "--marionette-computer-helper"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    try:
        ready = read_frame(proc.stdout)
        assert ready.get("kind") == "ready", f"first frame is not ready: {ready}"
        assert ready.get("protocol") == 1, f"protocol mismatch: {ready}"
        print("ok  ready frame:", ready)

        write_frame(
            proc.stdin,
            {"kind": "request", "id": 1, "op": "format-c-drive", "params": {}},
        )
        reply = read_frame(proc.stdout)
        assert reply["id"] == 1 and reply["ok"] is False, f"unknown op not refused: {reply}"
        assert "format-c-drive" in reply["error"]["message"], reply
        print("ok  unknown op refused:", reply["error"]["message"])

        write_frame(
            proc.stdin,
            {
                "kind": "request",
                "id": 2,
                "op": "configure",
                "params": {"driverPath": "Z:/definitely/not/here/cua-driver.exe"},
            },
        )
        reply = read_frame(proc.stdout)
        assert reply["id"] == 2 and reply["ok"] is False, f"bogus configure not refused: {reply}"
        assert "not on disk" in reply["error"]["message"], reply
        print("ok  bogus configure refused:", reply["error"]["message"])

        proc.stdin.close()
        code = proc.wait(timeout=15)
        assert code == 0, f"helper exit code {code}"
        print("ok  helper exits cleanly on stdin close")
        print("HELPER PROBE PASSED")
        return 0
    finally:
        if proc.poll() is None:
            proc.kill()


if __name__ == "__main__":
    sys.exit(main())
