// SPDX-License-Identifier: Apache-2.0
//! Context-pressure measurement over Claude Code and Codex session logs.

use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// An image block costs a flat cap in billed tokens, nothing like the length of
/// its base64 payload. Counting the payload puts `Read` at the top of the table
/// and is simply wrong — the first pass of this analysis fell for it.
const IMAGE_TOKENS: u64 = 1600;

/// Rough chars-per-token for prose and shell output. Good enough to rank tools;
/// not a billing figure.
const CHARS_PER_TOKEN: usize = 4;

#[derive(Args)]
pub struct ScanArgs {
    /// Claude Code projects directory (default: ~/.claude/projects)
    #[arg(long)]
    claude_dir: Option<PathBuf>,
    /// Codex sessions directory (default: ~/.codex/sessions)
    #[arg(long)]
    codex_dir: Option<PathBuf>,
    /// Emit JSON instead of a table
    #[arg(long)]
    json: bool,
    /// How many rows to show in each table
    #[arg(long, default_value_t = 12)]
    top: usize,
}

#[derive(Default, Serialize)]
struct ToolStat {
    calls: u64,
    raw_tokens: u64,
    pressure: u64,
    sizes: Vec<u64>,
}

#[derive(Serialize)]
struct SessionStat {
    id: String,
    turns: u64,
    billed_context: u64,
    output: u64,
}

#[derive(Serialize)]
struct Report {
    sessions: usize,
    assistant_turns: u64,
    user_turns: u64,
    billed_context: u64,
    billed_output: u64,
    total_pressure: u64,
    by_tool: Vec<ToolRow>,
    by_bucket: Vec<BucketRow>,
    top_sessions: Vec<SessionStat>,
}

#[derive(Serialize)]
struct ToolRow {
    tool: String,
    calls: u64,
    raw_tokens: u64,
    median_output: u64,
    p90_output: u64,
    max_output: u64,
    pressure: u64,
    share: f64,
}

#[derive(Serialize)]
struct BucketRow {
    bucket: &'static str,
    count: u64,
    pressure: u64,
    share: f64,
}

/// One event in a session, reduced to what pressure accounting needs.
enum Event {
    AssistantTurn,
    ToolResult { tool: String, tokens: u64 },
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn jsonl_files(root: &Path) -> Vec<PathBuf> {
    if !root.exists() {
        return Vec::new();
    }
    WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .map(|e| e.into_path())
        .collect()
}

/// Token cost of a tool result, counting image blocks at the flat cap rather
/// than by the size of their encoded payload.
fn result_tokens(content: &Value) -> u64 {
    match content {
        Value::String(s) => (s.len() / CHARS_PER_TOKEN) as u64,
        Value::Array(items) => items
            .iter()
            .map(|b| match b.get("type").and_then(Value::as_str) {
                Some("image") => IMAGE_TOKENS,
                Some("text") => {
                    let t = b.get("text").and_then(Value::as_str).unwrap_or("");
                    (t.len() / CHARS_PER_TOKEN) as u64
                }
                _ => (b.to_string().len() / CHARS_PER_TOKEN) as u64,
            })
            .sum(),
        other => (other.to_string().len() / CHARS_PER_TOKEN) as u64,
    }
}

fn bucket_of(tokens: u64) -> &'static str {
    match tokens * CHARS_PER_TOKEN as u64 {
        0..=999 => "0-1k",
        1000..=4999 => "1-5k",
        5000..=19999 => "5-20k",
        20000..=99999 => "20-100k",
        _ => "100k+",
    }
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx]
}

/// Walk one Claude Code session file, returning its events in order plus the
/// billed usage the provider actually reported.
fn parse_claude(path: &Path) -> Result<(Vec<Event>, u64, u64, u64)> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut events = Vec::new();
    let (mut ctx, mut out, mut user_turns) = (0u64, 0u64, 0u64);
    // tool_use id -> tool name, so a result can be attributed to its caller.
    let mut pending: HashMap<String, String> = HashMap::new();

    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        match v.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                events.push(Event::AssistantTurn);
                let msg = v.get("message");
                if let Some(u) = msg.and_then(|m| m.get("usage")) {
                    let g = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                    ctx += g("input_tokens")
                        + g("cache_read_input_tokens")
                        + g("cache_creation_input_tokens");
                    out += g("output_tokens");
                }
                if let Some(blocks) = msg.and_then(|m| m.get("content")).and_then(Value::as_array) {
                    for b in blocks {
                        if b.get("type").and_then(Value::as_str) == Some("tool_use")
                            && let (Some(id), Some(name)) = (
                                b.get("id").and_then(Value::as_str),
                                b.get("name").and_then(Value::as_str),
                            )
                        {
                            pending.insert(id.to_string(), name.to_string());
                        }
                    }
                }
            }
            Some("user") => {
                let content = v.get("message").and_then(|m| m.get("content"));
                match content {
                    Some(Value::Array(blocks)) => {
                        let mut had_result = false;
                        for b in blocks {
                            if b.get("type").and_then(Value::as_str) == Some("tool_result") {
                                had_result = true;
                                let tool = b
                                    .get("tool_use_id")
                                    .and_then(Value::as_str)
                                    .and_then(|id| pending.get(id))
                                    .cloned()
                                    .unwrap_or_else(|| "?".to_string());
                                let tokens = b.get("content").map(result_tokens).unwrap_or(0);
                                events.push(Event::ToolResult { tool, tokens });
                            }
                        }
                        if !had_result {
                            user_turns += 1;
                        }
                    }
                    Some(Value::String(_)) => user_turns += 1,
                    _ => {}
                }
            }
            _ => {}
        }
    }
    Ok((events, ctx, out, user_turns))
}

/// Codex logs a flatter shape: `function_call` items and cumulative token usage.
fn parse_codex(path: &Path) -> Result<(Vec<Event>, u64, u64, u64)> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut events = Vec::new();
    let (mut ctx, mut out, mut user_turns) = (0u64, 0u64, 0u64);
    let mut last_name = String::from("exec_command");

    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let p = v.get("payload").unwrap_or(&v);

        match p.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                events.push(Event::AssistantTurn);
                if let Some(n) = p.get("name").and_then(Value::as_str) {
                    last_name = n.to_string();
                }
            }
            Some("function_call_output") => {
                let tokens = p.get("output").map(result_tokens).unwrap_or(0);
                events.push(Event::ToolResult {
                    tool: last_name.clone(),
                    tokens,
                });
            }
            Some("message") if p.get("role").and_then(Value::as_str) == Some("user") => {
                user_turns += 1;
            }
            _ => {}
        }
        // Codex reports cumulative usage; keep the high-water mark per session.
        if let Some(info) = p.get("info")
            && let Some(tu) = info.get("total_token_usage")
        {
            let g = |k: &str| tu.get(k).and_then(Value::as_u64).unwrap_or(0);
            ctx = ctx.max(g("input_tokens"));
            out = out.max(g("output_tokens"));
        }
    }
    Ok((events, ctx, out, user_turns))
}

/// Pressure = tokens of a result multiplied by the number of model turns that
/// come after it and therefore re-read it.
fn accumulate(events: &[Event], tools: &mut HashMap<String, ToolStat>) -> u64 {
    let total_turns = events
        .iter()
        .filter(|e| matches!(e, Event::AssistantTurn))
        .count() as u64;
    let mut seen = 0u64;
    let mut session_pressure = 0u64;

    for e in events {
        match e {
            Event::AssistantTurn => seen += 1,
            Event::ToolResult { tool, tokens } => {
                let pressure = tokens * total_turns.saturating_sub(seen);
                session_pressure += pressure;
                let s = tools.entry(tool.clone()).or_default();
                s.calls += 1;
                s.raw_tokens += tokens;
                s.pressure += pressure;
                s.sizes.push(*tokens);
            }
        }
    }
    session_pressure
}

pub fn run(args: ScanArgs) -> Result<()> {
    let claude = args
        .claude_dir
        .unwrap_or_else(|| home().join(".claude/projects"));
    let codex = args
        .codex_dir
        .unwrap_or_else(|| home().join(".codex/sessions"));

    let mut tools: HashMap<String, ToolStat> = HashMap::new();
    let mut buckets: HashMap<&'static str, (u64, u64)> = HashMap::new();
    let mut sessions: Vec<SessionStat> = Vec::new();
    let (mut turns, mut user_turns, mut ctx_total, mut out_total, mut pressure_total) =
        (0u64, 0u64, 0u64, 0u64, 0u64);

    let work: Vec<(PathBuf, bool)> = jsonl_files(&claude)
        .into_iter()
        .map(|p| (p, true))
        .chain(jsonl_files(&codex).into_iter().map(|p| (p, false)))
        .collect();

    if work.is_empty() {
        anyhow::bail!(
            "no session logs found in {} or {}",
            claude.display(),
            codex.display()
        );
    }

    for (path, is_claude) in work {
        let parsed = if is_claude {
            parse_claude(&path)
        } else {
            parse_codex(&path)
        };
        let Ok((events, ctx, out, users)) = parsed else {
            continue;
        };

        let t = events
            .iter()
            .filter(|e| matches!(e, Event::AssistantTurn))
            .count() as u64;
        if t == 0 {
            continue;
        }

        for e in &events {
            if let Event::ToolResult { tokens, .. } = e {
                let b = buckets.entry(bucket_of(*tokens)).or_insert((0, 0));
                b.0 += 1;
            }
        }
        let sp = accumulate(&events, &mut tools);
        // Second pass for bucket pressure, now that turn counts are known.
        let total_turns = t;
        let mut seen = 0u64;
        for e in &events {
            match e {
                Event::AssistantTurn => seen += 1,
                Event::ToolResult { tokens, .. } => {
                    let b = buckets.entry(bucket_of(*tokens)).or_insert((0, 0));
                    b.1 += tokens * total_turns.saturating_sub(seen);
                }
            }
        }

        turns += t;
        user_turns += users;
        ctx_total += ctx;
        out_total += out;
        pressure_total += sp;
        sessions.push(SessionStat {
            id: path
                .file_stem()
                .map(|s| s.to_string_lossy().chars().take(8).collect())
                .unwrap_or_default(),
            turns: t,
            billed_context: ctx,
            output: out,
        });
    }

    let session_count = sessions.len();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.billed_context));
    sessions.truncate(args.top);

    let mut by_tool: Vec<ToolRow> = tools
        .into_iter()
        .map(|(tool, mut s)| {
            s.sizes.sort_unstable();
            ToolRow {
                tool,
                calls: s.calls,
                raw_tokens: s.raw_tokens,
                median_output: percentile(&s.sizes, 0.5),
                p90_output: percentile(&s.sizes, 0.9),
                max_output: s.sizes.last().copied().unwrap_or(0),
                pressure: s.pressure,
                share: if pressure_total > 0 {
                    100.0 * s.pressure as f64 / pressure_total as f64
                } else {
                    0.0
                },
            }
        })
        .collect();
    by_tool.sort_by_key(|r| std::cmp::Reverse(r.pressure));

    let order = ["0-1k", "1-5k", "5-20k", "20-100k", "100k+"];
    let by_bucket: Vec<BucketRow> = order
        .iter()
        .map(|b| {
            let (count, pressure) = buckets.get(b).copied().unwrap_or((0, 0));
            BucketRow {
                bucket: b,
                count,
                pressure,
                share: if pressure_total > 0 {
                    100.0 * pressure as f64 / pressure_total as f64
                } else {
                    0.0
                },
            }
        })
        .collect();

    let report = Report {
        sessions: session_count,
        assistant_turns: turns,
        user_turns,
        billed_context: ctx_total,
        billed_output: out_total,
        total_pressure: pressure_total,
        by_tool,
        by_bucket,
        top_sessions: sessions,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_table(&report, args.top);
    }
    Ok(())
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

fn print_table(r: &Report, top: usize) {
    println!(
        "\nSessions {}   model turns {}   your turns {}",
        r.sessions,
        thousands(r.assistant_turns),
        thousands(r.user_turns)
    );
    println!(
        "Billed context {}   output {}",
        thousands(r.billed_context),
        thousands(r.billed_output)
    );
    if r.user_turns > 0 {
        println!(
            "Model turns per question: {:.1}",
            r.assistant_turns as f64 / r.user_turns as f64
        );
    }
    if let Some(per_turn) = r.billed_context.checked_div(r.assistant_turns) {
        println!("Context per model turn:   {}", thousands(per_turn));
    }

    println!("\nContext pressure by tool  (result tokens x later turns that re-read them)");
    println!(
        "{:<38}{:>9}{:>14}{:>10}{:>12}{:>17}{:>9}",
        "tool", "calls", "raw tok", "median", "p90", "pressure", "share"
    );
    for row in r.by_tool.iter().take(top) {
        println!(
            "{:<38}{:>9}{:>14}{:>10}{:>12}{:>17}{:>8.1}%",
            row.tool,
            thousands(row.calls),
            thousands(row.raw_tokens),
            thousands(row.median_output),
            thousands(row.p90_output),
            thousands(row.pressure),
            row.share
        );
    }

    println!("\nBy result size");
    println!(
        "{:<12}{:>10}{:>16}{:>8}",
        "bucket", "count", "pressure", "share"
    );
    for b in &r.by_bucket {
        println!(
            "{:<12}{:>10}{:>18}{:>8.1}%",
            b.bucket,
            thousands(b.count),
            thousands(b.pressure),
            b.share
        );
    }

    println!("\nHeaviest sessions");
    println!(
        "{:<12}{:>8}{:>16}{:>12}",
        "session", "turns", "context", "output"
    );
    for s in &r.top_sessions {
        println!(
            "{:<12}{:>9}{:>18}{:>14}",
            s.id,
            thousands(s.turns),
            thousands(s.billed_context),
            thousands(s.output)
        );
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn image_blocks_cost_the_flat_cap_not_their_payload() {
        // The base64 payload of a screenshot is enormous; billing is not.
        let huge = "A".repeat(400_000);
        let content = json!([{ "type": "image", "source": { "data": huge } }]);
        assert_eq!(result_tokens(&content), IMAGE_TOKENS);
    }

    #[test]
    fn text_blocks_count_by_length() {
        let content = json!([{ "type": "text", "text": "x".repeat(4000) }]);
        assert_eq!(result_tokens(&content), 1000);
    }

    #[test]
    fn pressure_weights_early_results_more_than_late_ones() {
        // Same 100-token result: once at the start of a 3-turn session, once at
        // the end. The early one is re-read twice, the late one never.
        let early = vec![
            Event::ToolResult {
                tool: "Bash".into(),
                tokens: 100,
            },
            Event::AssistantTurn,
            Event::AssistantTurn,
            Event::AssistantTurn,
        ];
        let late = vec![
            Event::AssistantTurn,
            Event::AssistantTurn,
            Event::AssistantTurn,
            Event::ToolResult {
                tool: "Bash".into(),
                tokens: 100,
            },
        ];
        let mut t1 = HashMap::new();
        let mut t2 = HashMap::new();
        assert_eq!(accumulate(&early, &mut t1), 300);
        assert_eq!(accumulate(&late, &mut t2), 0);
    }

    #[test]
    fn buckets_split_on_character_length() {
        assert_eq!(bucket_of(100), "0-1k"); // 400 chars
        assert_eq!(bucket_of(400), "1-5k"); // 1600 chars
        assert_eq!(bucket_of(5000), "20-100k");
    }
}
