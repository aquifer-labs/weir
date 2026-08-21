# Weir

**Keep working in Claude Code and Codex.**

Weir stops bloated tool results before they reach the model: it bounds SQL and MCP
queries, caps shell output, and blocks forbidden calls through the harnesses' own
pre-tool hooks. Every change gets a verifiable receipt and a measured token saving.
One `init` — no new UI, no new sessions, no new agent loop.

Weir is not an agent harness. It is a thin layer on top of the one you already use.

## Why

Measured on a real 17-day Claude Code history: 11.24 billion context tokens,
25,083 assistant turns for 1,044 user turns — about 24 agent iterations per
question, averaging 444,539 context tokens per turn. The cost is structural:
turns multiplied by an accumulating context, not the size of any single output.

Weir attacks that with deterministic gates, because deterministic I/O gates
transfer across models and prompts do not.

## Status

Early. `weir scan` (measurement) is the first thing that works, deliberately:
without a baseline, no claim about savings is checkable.

## Install

    cargo install --path crates/weir-cli

## Use

    weir scan                 # measure context pressure in local agent logs
    weir scan --json          # machine-readable

## License

Apache-2.0. Part of [Aquifer Labs](https://github.com/aquifer-labs) —
alongside [Artesian](https://github.com/aquifer-labs/artesian) (memory) and
[OpenHavn](https://github.com/aquifer-labs/openhavn) (fleet).
