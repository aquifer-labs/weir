# Weir

**Keep working in Claude Code and Codex.**

Weir stops bloated tool results before they reach the model: it bounds SQL and MCP
queries, caps shell output, and blocks forbidden calls through the harnesses' own
pre-tool hooks. Every change gets a verifiable receipt and a measured token saving.
One `init` — no new UI, no new sessions, no new agent loop.

Weir is not an agent harness. It is a thin layer on top of the one you already use.

## The metric: pressure, not size

A tool result does not cost you its own tokens once. It costs them again on every
later model turn that re-reads the accumulated context. So:

    pressure = result tokens x model turns that follow it

This reorders everything. A 300-token shell result early in a long session
outweighs a 30,000-token one at the very end. Ranking tools by raw output — the
obvious thing to do — points at the wrong targets.

Two corollaries that fall out of measuring real logs:

- The cost is **structural**: many turns multiplied by a growing context, not any
  single fat output. Shaving a few hundred tokens off a typical shell call is
  nearly pointless; the mid-size band is usually where the mass sits.
- Images must be counted at the flat billed cap, **not** at the length of their
  base64 payload. Counting the payload puts file reads at the top of the table and
  is simply wrong.

`weir scan` computes this over your own logs so you can see your distribution
before changing anything. Measure first: without a baseline, no claim about
savings is checkable — including ours.

## Status

Early. `weir scan` works. Gating is next, and lands in shadow mode first: it logs
what it *would* have changed, so the effect can be measured before anything is
actually altered.

## Install

    cargo install --path crates/weir-cli

## Use

    weir scan                 # measure context pressure in local agent logs
    weir scan --json          # machine-readable
    weir scan --top 20        # more rows

Reads `~/.claude/projects` and `~/.codex/sessions` by default; override with
`--claude-dir` / `--codex-dir`. Nothing is sent anywhere — it is a local read.

## Design rules

- **Deterministic gates, not model calls.** Deterministic I/O gates transfer
  across models; prompts do not. A local model in the hot path adds latency and
  a failure mode for no reliable gain.
- **Never truncate an error.** Stack traces and failure output pass through whole.
- **Fail open.** A broken rule degrades to no effect, never to a broken agent.
- **Measure end to end.** A filter that halves output but adds a retry made things
  worse. Turn count and task completion are part of the metric.

## License

Apache-2.0. Part of [Aquifer Labs](https://github.com/aquifer-labs) —
alongside [Artesian](https://github.com/aquifer-labs/artesian) (memory) and
[OpenHavn](https://github.com/aquifer-labs/openhavn) (fleet).
