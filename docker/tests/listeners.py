#!/usr/bin/env python3
"""Check a container's listening sockets from its /proc/net tables.

    docker compose exec -T pumpkin cat /proc/net/tcp /proc/net/tcp6 \
        /proc/net/udp /proc/net/udp6 | python3 listeners.py

Reads the concatenated tables on stdin. Fails unless TCP 25565 is the ONLY
listening TCP socket and nothing is bound on UDP 25565 (query) or UDP 19132
(Bedrock).

/proc/net lists the whole network namespace, not just the server process. On a
compose (user-defined) network Docker runs its embedded DNS resolver inside
that namespace on 127.0.0.11, on a random TCP port and a random UDP port. Those
two sockets are Docker's, loopback-only and unreachable from outside, so
sockets bound to exactly 127.0.0.11 are reported but not counted. Everything
else, including any other loopback address, still counts.
"""

import sys

TCP_LISTEN = "0A"
# 127.0.0.11 as /proc/net/tcp writes it: the IPv4 address as a little-endian
# hex word.
DOCKER_DNS = "0B00007F"


def main() -> int:
    tcp_listen: set[int] = set()
    udp_bound: set[int] = set()
    docker_dns: set[str] = set()
    table = None
    for line in sys.stdin:
        fields = line.split()
        if not fields:
            continue
        if fields[0] == "sl":
            # Header row; tcp tables have "tx_queue rx_queue tr tm->when",
            # udp ones too, so tell them apart by the "drops" column.
            table = "udp" if "drops" in fields else "tcp"
            continue
        if table is None or len(fields) < 4 or ":" not in fields[1]:
            continue
        addr, port_hex = fields[1].rsplit(":", 1)
        port = int(port_hex, 16)
        if addr.upper() == DOCKER_DNS:
            if table == "udp" or fields[3] == TCP_LISTEN:
                docker_dns.add(f"{table}/{port}")
            continue
        if table == "tcp" and fields[3] == TCP_LISTEN:
            tcp_listen.add(port)
        elif table == "udp":
            udp_bound.add(port)

    print(f"TCP listening: {sorted(tcp_listen)}")
    print(f"UDP bound:     {sorted(udp_bound)}")
    print(f"Docker DNS (127.0.0.11, not counted): {sorted(docker_dns)}")
    errors = []
    if tcp_listen != {25565}:
        errors.append(f"expected only TCP 25565 to listen, got {sorted(tcp_listen)}")
    for port, what in ((25565, "query"), (19132, "Bedrock")):
        if port in udp_bound:
            errors.append(f"UDP {port} is bound ({what} should be off)")
    for err in errors:
        print(f"FAIL: {err}", file=sys.stderr)
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
