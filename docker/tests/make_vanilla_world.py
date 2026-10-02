#!/usr/bin/env python3
"""Generate a small vanilla Java Edition world with Mojang's own server jar.

    python3 make_vanilla_world.py OUT_DIR [--version 26.3] [--seed 26326]

Downloads the official dedicated server jar for VERSION (SHA-1 checked against
Mojang's version manifest), starts it once in a scratch directory so it
generates the spawn area, stops it cleanly with the `stop` console command
and copies the resulting `world` folder to OUT_DIR. Needs Java 25+ on PATH.
Running the jar accepts the Minecraft EULA for this throwaway test server.
"""

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from pathlib import Path

MANIFEST = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json"


def fetch_json(url: str) -> dict:
    with urllib.request.urlopen(url, timeout=60) as resp:
        return json.load(resp)


def download_server(version: str, dest: Path) -> None:
    manifest = fetch_json(MANIFEST)
    entry = next((v for v in manifest["versions"] if v["id"] == version), None)
    if entry is None:
        raise SystemExit(f"version {version} not in the Mojang manifest")
    server = fetch_json(entry["url"])["downloads"]["server"]
    with urllib.request.urlopen(server["url"], timeout=300) as resp:
        data = resp.read()
    digest = hashlib.sha1(data).hexdigest()
    if digest != server["sha1"]:
        raise SystemExit(f"server.jar sha1 {digest} != manifest {server['sha1']}")
    dest.write_bytes(data)
    print(f"downloaded {version} server.jar ({len(data)} bytes, sha1 ok)")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("out_dir", type=Path)
    parser.add_argument("--version", default="26.3")
    parser.add_argument("--seed", default="26326")
    parser.add_argument("--port", type=int, default=25599)
    parser.add_argument("--timeout", type=float, default=600)
    args = parser.parse_args()

    if args.out_dir.exists():
        raise SystemExit(f"{args.out_dir} already exists")

    with tempfile.TemporaryDirectory(prefix="vanilla-world-") as tmp:
        work = Path(tmp)
        download_server(args.version, work / "server.jar")
        (work / "eula.txt").write_text("eula=true\n")
        (work / "server.properties").write_text(
            "\n".join(
                [
                    f"level-seed={args.seed}",
                    "level-name=world",
                    f"server-port={args.port}",
                    "online-mode=false",
                    "enable-query=false",
                    "enable-rcon=false",
                    "view-distance=4",
                    "simulation-distance=4",
                    "spawn-protection=0",
                ]
            )
            + "\n"
        )

        proc = subprocess.Popen(
            ["java", "-Xms512M", "-Xmx2G", "-jar", "server.jar", "--nogui"],
            cwd=work,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        ready = threading.Event()

        def pump() -> None:
            assert proc.stdout is not None
            for line in proc.stdout:
                sys.stdout.write("[vanilla] " + line)
                if "Done (" in line and 'For help, type "help"' in line:
                    ready.set()

        reader = threading.Thread(target=pump, daemon=True)
        reader.start()
        deadline = time.monotonic() + args.timeout
        while not ready.is_set():
            if proc.poll() is not None:
                raise SystemExit(f"vanilla server exited early with {proc.returncode}")
            if time.monotonic() > deadline:
                proc.kill()
                raise SystemExit("vanilla server did not finish starting in time")
            time.sleep(1)

        assert proc.stdin is not None
        proc.stdin.write("stop\n")
        proc.stdin.flush()
        if proc.wait(timeout=180) != 0:
            raise SystemExit(f"vanilla server stopped with {proc.returncode}")
        reader.join(timeout=10)

        world = work / "world"
        if not (world / "level.dat").is_file():
            raise SystemExit("vanilla server produced no world/level.dat")
        regions = list(world.rglob("*.mca"))
        if not regions:
            raise SystemExit("vanilla server produced no region files")
        shutil.copytree(world, args.out_dir)
        (args.out_dir / "session.lock").unlink(missing_ok=True)
        print(f"world copied to {args.out_dir} ({len(regions)} region files)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
