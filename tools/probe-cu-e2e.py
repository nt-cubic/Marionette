"""End-to-end helper probe: configure with the REAL installed driver, then
list windows through helper -> cua-driver. Prints the raw reply.

Usage: python tools/probe-cu-e2e.py
"""

import json
import struct
import subprocess
import sys
import time

EXE = r"D:\Projects\Marionette\release\0.5.6\Marionette.exe"
DRIVER = r"C:\Users\NT's Station\.marionette\cua\0.36.0\cua-driver.exe"


def read_frame(stream, timeout=60.0):
    deadline = time.time() + timeout
    header = stream.read(4)
    if len(header) != 4:
        raise AssertionError("no header")
    (length,) = struct.unpack("<I", header)
    payload = b""
    while len(payload) < length and time.time() < deadline:
        chunk = stream.read(length - len(payload))
        if not chunk:
            break
        payload += chunk
    return json.loads(payload.decode("utf-8"))


def write_frame(stream, frame):
    payload = json.dumps(frame).encode("utf-8")
    stream.write(struct.pack("<I", len(payload)))
    stream.write(payload)
    stream.flush()


def main():
    proc = subprocess.Popen(
        [EXE, "--marionette-computer-helper"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        env = dict(**__import__("os").environ)
        ready = read_frame(proc.stdout, 20)
        print("ready:", ready)

        write_frame(
            proc.stdin,
            {"kind": "request", "id": 1, "op": "configure", "params": {"driverPath": DRIVER}},
        )
        reply = read_frame(proc.stdout, 60)
        print("configure:", json.dumps(reply, ensure_ascii=False)[:200])

        t0 = time.time()
        write_frame(proc.stdin, {"kind": "request", "id": 2, "op": "list-windows", "params": {}})
        reply = read_frame(proc.stdout, 60)
        print(f"list-windows ({time.time() - t0:.1f}s):")
        print(json.dumps(reply, ensure_ascii=False)[:1200])
        return 0
    finally:
        proc.kill()
        err = proc.stderr.read().decode("utf-8", "replace")[:500]
        if err.strip():
            print("helper stderr:", err)


if __name__ == "__main__":
    sys.exit(main())
