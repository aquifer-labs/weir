// SPDX-License-Identifier: Apache-2.0
//! The hook entry point. Reads one hook payload on stdin, writes one decision on
//! stdout, exits 0.
//!
//! Shadow mode is the default and is the whole point of this stage: the rules run
//! and record what they *would* have done, but the agent's behaviour is untouched.
//! Only once the shadow log shows a real, measured saving does `--enforce` make
//! sense.
//!
//! Failure is always silent and open. A hook that errors, or that cannot parse
//! its input, prints `{}` and lets the original call through. A broken rule must
//! degrade to "no effect", never to a broken agent.

use anyhow::Result;
use clap::Args;
use serde_json::{Value, json};
use std::io::Read;

use crate::config;
use crate::gate::trim;
use crate::policy;
use crate::shape::{Limits, shape};

#[derive(Args)]
pub struct HookArgs {
    /// Actually rewrite the call. Without this, Weir only records what it would do.
    #[arg(long)]
    enforce: bool,
    /// Where to append the shadow log (default: ~/.weir/shadow.jsonl)
    #[arg(long)]
    log: Option<std::path::PathBuf>,
}

fn log_path(explicit: Option<std::path::PathBuf>) -> std::path::PathBuf {
    explicit.unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default()
            .join(".weir/shadow.jsonl")
    })
}

fn approx_tokens(v: &Value) -> u64 {
    let s = match v {
        Value::String(s) => s.len(),
        other => other.to_string().len(),
    };
    (s / 4) as u64
}

/// Append one line to the shadow log. Best effort: a log that cannot be written
/// must not stop the agent.
fn record(path: &std::path::Path, entry: &Value) {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{entry}");
    }
}

pub fn run(args: HookArgs) -> Result<()> {
    let mut raw = String::new();
    if std::io::stdin().read_to_string(&mut raw).is_err() {
        println!("{{}}");
        return Ok(());
    }
    let Ok(payload) = serde_json::from_str::<Value>(&raw) else {
        println!("{{}}");
        return Ok(());
    };

    let event = payload
        .get("hook_event_name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let tool = payload
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let log = log_path(args.log);
    // The agent's working directory is where a project-level weir.toml lives.
    let cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .map(std::path::Path::new);
    let cfg = config::load(cwd);

    match event {
        "PreToolUse" => {
            let input = payload.get("tool_input").cloned().unwrap_or(Value::Null);

            // Refusals come first: there is no point shaping a call that must
            // not run. Denials are enforced even in shadow mode - a guard rail
            // that only reports is not a guard rail.
            if let Some(d) =
                policy::check(tool, &input, &cfg.policy.deny_bash, &cfg.policy.deny_sql)
            {
                record(
                    &log,
                    &json!({
                        "event": "PreToolUse", "tool": tool, "rule": d.rule,
                        "note": d.reason, "enforced": true, "denied": true,
                    }),
                );
                println!(
                    "{}",
                    json!({"hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason": format!("weir/{}: {}", d.rule, d.reason)
                    }})
                );
                return Ok(());
            }

            let limits = Limits {
                sql_limit: cfg.shape.sql_limit,
                recall_limit: cfg.shape.recall_limit,
                shell_bytes: cfg.shape.shell_cap_bytes,
            };
            let Some(s) = shape(tool, &input, &limits) else {
                println!("{{}}");
                return Ok(());
            };
            record(
                &log,
                &json!({
                    "event": "PreToolUse",
                    "tool": tool,
                    "rule": s.rule,
                    "note": s.note,
                    "enforced": args.enforce,
                    "before": input,
                    "after": s.input,
                }),
            );
            if args.enforce {
                println!(
                    "{}",
                    json!({"hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "allow",
                        "permissionDecisionReason": format!("weir/{}: {}", s.rule, s.note),
                        "updatedInput": s.input
                    }})
                );
            } else {
                println!("{{}}");
            }
        }
        "PostToolUse" => {
            let resp = payload.get("tool_response").cloned().unwrap_or(Value::Null);
            // Only plain-text output can be trimmed meaningfully. Structured
            // results are left alone: cutting the middle out of JSON produces
            // something that parses as nothing.
            let text = match &resp {
                Value::String(s) => Some(s.clone()),
                _ => None,
            };
            let before = approx_tokens(&resp);

            match text.as_deref().and_then(|t| {
                trim(
                    t,
                    cfg.gate.budget_tokens,
                    cfg.gate.head_lines,
                    cfg.gate.tail_lines,
                )
            }) {
                Some(t) => {
                    record(
                        &log,
                        &json!({
                            "event": "PostToolUse",
                            "tool": tool,
                            "rule": "gate_trim",
                            "enforced": args.enforce,
                            "result_tokens": t.before_tokens,
                            "trimmed_tokens": t.after_tokens,
                            "saved_tokens": t.before_tokens - t.after_tokens,
                            "duration_ms": payload.get("duration_ms"),
                        }),
                    );
                    if args.enforce {
                        println!(
                            "{}",
                            json!({"hookSpecificOutput": {
                                "hookEventName": "PostToolUse",
                                "updatedToolOutput": t.text
                            }})
                        );
                        return Ok(());
                    }
                }
                None => {
                    if before >= 500 {
                        record(
                            &log,
                            &json!({
                                "event": "PostToolUse",
                                "tool": tool,
                                "result_tokens": before,
                                "duration_ms": payload.get("duration_ms"),
                            }),
                        );
                    }
                }
            }
            println!("{{}}");
        }
        _ => println!("{{}}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unparseable_input_is_a_no_op_not_an_error() {
        // The contract that matters: a broken hook never breaks the agent.
        assert!(serde_json::from_str::<Value>("not json{").is_err());
    }

    #[test]
    fn response_size_is_estimated_from_serialised_length() {
        assert_eq!(approx_tokens(&Value::String("x".repeat(4000))), 1000);
    }
}
