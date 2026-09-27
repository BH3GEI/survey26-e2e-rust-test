#!/usr/bin/env python3
"""Platform entry point (starter-kit compatible): build and exec the Rust agent.

The decision logic lives in Rust (src/main.rs). This shim only locates or
builds the binary and replaces itself with it, forwarding stdio untouched.
"""
import os
import shutil
import subprocess
import sys
from pathlib import Path

AGENT_DIR = Path(__file__).resolve().parent
ROOT = AGENT_DIR.parent
BIN = ROOT / "target" / "release" / ("survey26_agent.exe" if os.name == "nt" else "survey26_agent")


def _tool(name):
    found = shutil.which(name)
    if found:
        return found
    for base in (Path.home() / ".cargo" / "bin", Path("/usr/local/bin"), Path("/opt/homebrew/bin"), Path("/usr/bin")):
        candidate = base / name
        if candidate.is_file():
            return str(candidate)
    return None


def ensure_binary():
    if BIN.is_file():
        return
    build_env = dict(os.environ)
    build_env.setdefault("CARGO_HOME", str(ROOT / "target" / "cargo-home"))
    build_env.setdefault("CARGO_TARGET_DIR", str(ROOT / "target"))
    build_env.setdefault("CARGO_NET_OFFLINE", "true")
    rustc = _tool("rustc")
    if rustc:
        BIN.parent.mkdir(parents=True, exist_ok=True)
        result = subprocess.run(
            [rustc, "-O", "--edition=2021", str(ROOT / "src" / "main.rs"), "-o", str(BIN)],
            cwd=str(ROOT), env=build_env,
        )
        if result.returncode == 0 and BIN.is_file():
            return
        sys.stderr.write("rust-agent: rustc build failed, trying cargo\n")
    cargo = _tool("cargo")
    if cargo:
        result = subprocess.run([cargo, "build", "--release", "--offline"], cwd=str(ROOT), env=build_env)
        if result.returncode != 0:
            result = subprocess.run([cargo, "build", "--release"], cwd=str(ROOT), env=build_env)
        if result.returncode == 0 and BIN.is_file():
            return
    sys.stderr.write("rust-agent: could not build the Rust binary (no rustc/cargo found)\n")
    sys.exit(3)


def main():
    ensure_binary()
    os.execv(str(BIN), [str(BIN)])


if __name__ == "__main__":
    main()
