# survey26-e2e-rust-test

Deterministic Agent Observer entry written in **pure Rust** - no crates, no model calls.
The agent speaks participant-agent-protocol-v1/v2 (JSON Lines over stdin/stdout) and
implements the public challenge-score-v3 preview ranking plus conservative anomaly
reporting (NOVA / Reddening / Instrument_Failure) for decision-snapshot-v3 mechanics.

## Layout

- src/main.rs - the whole agent: JSON parser, protocol loop, scoring preview
  (port of the starter kit's agent/scoring_preview.py) and the anomaly detector
  (port of agent/anomaly_detection.py).
- agent/minimal_agent.py - platform entry shim: builds the Rust binary with
  rustc -O (falling back to cargo build --release) if needed, then execs it
  with stdio forwarded. All logs go to stderr.

## Build and run

    cargo build --release            # or: rustc -O --edition=2021 src/main.rs -o survey26_agent
    ./target/release/survey26_agent  # speaks JSON Lines on stdin/stdout

## Local evaluation with the starter kit

    python3 local_runner.py --scenario scenarios/dev-reference         --agent /path/to/this/repo/agent/minimal_agent.py --wallclock 600 --out run_output
