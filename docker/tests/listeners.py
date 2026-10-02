#!/usr/bin/env python3
"""Check a container's listening sockets from its /proc/net tables.

    docker compose exec -T pumpkin cat /proc/net/tcp /proc/net/tcp6 \
        /proc/net/udp /proc/net/udp6 | python3 listeners.py

Reads the concatenated tables on stdin. Fails unless TCP 25565 is the ONLY
listening TCP socket and nothing is bound on UDP 25565 (query) or UDP 19132
(Bedrock).
"""

import sys

TCP_LISTEN = "0A"


def main() -> int:
    tcp_listen: set[int] = set()
    udp_bound: set[int] = set()
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
        port = int(fields[1].rsplit(":", 1)[1], 16)
        if table == "tcp" and fields[3] == TCP_LISTEN:
            tcp_listen.add(port)
        elif table == "udp":
            udp_bound.add(port)

    print(f"TCP listening: {sorted(tcp_listen)}")
    print(f"UDP bound:     {sorted(udp_bound)}")
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
