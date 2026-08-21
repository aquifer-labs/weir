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

## What it does

Two levers, because the harnesses expose two:

- **Shape** (`PreToolUse` → `updatedInput`) bounds a request whose output size is
  predictable: a `SELECT` with no `LIMIT`, a recall with a large default, a bare
  `cat`. Cheaper not to produce the data than to produce and discard it.
- **Gate** (`PostToolUse` → `updatedToolOutput`) handles the rest. A shell command
  can return 50 tokens or 7,000 and there is no way to know beforehand, so this
  looks at the real output, keeps the head and the tail, and says what it cut.

Both start in **shadow mode**: they record what they would have done and change
nothing. Arm them with `--enforce` once the numbers justify it.

## Status

Early, but working end to end on both harnesses.

## Install

Weir is two halves, and they update independently — that is the one thing worth
understanding before installing it.

- The **binary** does the work. Install it with cargo or brew.
- The **plugin** is just the hook manifests that tell Claude Code and Codex to
  call the binary. It reaches you through each harness's marketplace.

Install the binary:

    cargo install --git https://github.com/aquifer-labs/weir weir-cli

Then wire it into the harnesses:

    weir init                                       # Claude Code, direct
    codex plugin marketplace add aquifer-labs/weir  # Codex, as a plugin
    codex plugin add weir --marketplace weir

Codex gates hooks behind persisted trust: start `codex` once interactively and
approve them, or it will skip them without saying so.

## Updating

Update both halves, then check they agree:

    cargo install --git https://github.com/aquifer-labs/weir weir-cli --force
    codex plugin marketplace upgrade weir
    weir doctor

`weir doctor` exists for exactly this: it prints the binary version, the plugin
version each harness has, whether the hooks are registered and trusted, and
which config is in effect. A mismatch is the ordinary failure here and it is
silent — the hooks keep firing, they just do the wrong thing. CI refuses to
publish a release whose manifests and crate version disagree, so a mismatch can
only come from a half-finished update on your side.

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
