#!/usr/bin/env python3
"""Minecraft Java status ping (Server List Ping), used by the Docker smoke test.

    python3 mc_status.py HOST PORT [--protocol N] [--expect-protocol N]
                         [--timeout SECS] [--retry-for SECS]

Prints the status JSON and exits 0 when the server answers a status request,
echoes the ping payload and (with --expect-protocol) reports that protocol.
Standard library only. 777 is Java Edition 26.3.
"""

import argparse
import json
import socket
import struct
import sys
import time


def varint(value: int) -> bytes:
    out = b""
    value &= 0xFFFFFFFF
    while True:
        byte = value & 0x7F
        value >>= 7
        if value:
            out += bytes([byte | 0x80])
        else:
            return out + bytes([byte])


def read_exact(sock: socket.socket, n: int) -> bytes:
    buf = b""
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk:
            raise ConnectionError("connection closed mid-packet")
        buf += chunk
    return buf


def read_varint(sock: socket.socket) -> int:
    result = 0
    for shift in range(0, 35, 7):
        byte = read_exact(sock, 1)[0]
        result |= (byte & 0x7F) << shift
        if not byte & 0x80:
            return result
    raise ValueError("varint too long")


def packet(packet_id: int, payload: bytes) -> bytes:
    body = varint(packet_id) + payload
    return varint(len(body)) + body


def status(host: str, port: int, protocol: int, timeout: float) -> dict:
    with socket.create_connection((host, port), timeout=timeout) as sock:
        sock.settimeout(timeout)
        addr = host.encode()
        # Handshake, next state 1 (status).
        handshake = varint(protocol) + varint(len(addr)) + addr + struct.pack(">H", port) + varint(1)
        sock.sendall(packet(0x00, handshake) + packet(0x00, b""))

        read_varint(sock)  # packet length
        if read_varint(sock) != 0x00:
            raise ValueError("expected status response packet 0x00")
        raw = read_exact(sock, read_varint(sock))
        info = json.loads(raw.decode("utf-8"))

        token = int(time.time() * 1000) & 0x7FFFFFFFFFFFFFFF
        sock.sendall(packet(0x01, struct.pack(">q", token)))
        read_varint(sock)
        if read_varint(sock) != 0x01:
            raise ValueError("expected pong packet 0x01")
        (echo,) = struct.unpack(">q", read_exact(sock, 8))
        if echo != token:
            raise ValueError(f"pong payload {echo} != ping payload {token}")
        return info


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("host")
    parser.add_argument("port", type=int)
    parser.add_argument("--protocol", type=int, default=777)
    parser.add_argument("--expect-protocol", type=int)
    parser.add_argument("--timeout", type=float, default=5.0)
    parser.add_argument("--retry-for", type=float, default=0.0)
    args = parser.parse_args()

    deadline = time.monotonic() + args.retry_for
    while True:
        try:
            info = status(args.host, args.port, args.protocol, args.timeout)
            print(json.dumps(info, indent=2))
            if args.expect_protocol is not None:
                got = info.get("version", {}).get("protocol")
                if got != args.expect_protocol:
                    print(f"server reports protocol {got}, expected {args.expect_protocol}", file=sys.stderr)
                    return 1
            return 0
        except (OSError, ValueError, ConnectionError) as err:
            if time.monotonic() >= deadline:
                print(f"status ping to {args.host}:{args.port} failed: {err}", file=sys.stderr)
                return 1
            time.sleep(2)


if __name__ == "__main__":
    sys.exit(main())
