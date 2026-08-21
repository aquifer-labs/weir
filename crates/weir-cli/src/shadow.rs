// SPDX-License-Identifier: Apache-2.0
//! Report what shadow mode observed, so a rule can be judged before it is armed.

use anyhow::Result;
use clap::Args;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

#[derive(Args)]
pub struct ShadowArgs {
    /// Shadow log to read (default: ~/.weir/shadow.jsonl)
    #[arg(long)]
    log: Option<PathBuf>,
}

#[derive(Default)]
struct RuleStat {
    fired: u64,
    tools: HashMap<String, u64>,
}

pub fn run(args: ShadowArgs) -> Result<()> {
    let path = args.log.unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(".weir/shadow.jsonl")
    });
    let Ok(file) = std::fs::File::open(&path) else {
        println!(
            "No shadow log at {}.\nRun `weir init`, do some real work, then come back.",
            path.display()
        );
        return Ok(());
    };

    let mut rules: HashMap<String, RuleStat> = HashMap::new();
    let mut observed: HashMap<String, (u64, u64)> = HashMap::new(); // tool -> (calls, tokens)

    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let tool = v
            .get("tool")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        match v.get("event").and_then(Value::as_str) {
            Some("PreToolUse") => {
                let rule = v
                    .get("rule")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_string();
                let e = rules.entry(rule).or_default();
                e.fired += 1;
                *e.tools.entry(tool).or_default() += 1;
            }
            Some("PostToolUse") => {
                let t = v.get("result_tokens").and_then(Value::as_u64).unwrap_or(0);
                let e = observed.entry(tool).or_insert((0, 0));
                e.0 += 1;
                e.1 += t;
            }
            _ => {}
        }
    }

    if rules.is_empty() && observed.is_empty() {
        println!("Shadow log is empty. Nothing matched yet.");
        return Ok(());
    }

    println!("\nRules that fired  (shadow — nothing was actually changed)");
    if rules.is_empty() {
        println!("  none");
    } else {
        let mut rows: Vec<_> = rules.into_iter().collect();
        rows.sort_by_key(|(_, s)| std::cmp::Reverse(s.fired));
        for (rule, s) in rows {
            let mut tools: Vec<_> = s.tools.into_iter().collect();
            tools.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
            let where_ = tools
                .iter()
                .take(3)
                .map(|(t, n)| format!("{t} x{n}"))
                .collect::<Vec<_>>()
                .join(", ");
            println!("  {:<16} {:>5}   {}", rule, s.fired, where_);
        }
    }

    println!("\nLarge results seen  (>=500 tokens)");
    let mut rows: Vec<_> = observed.into_iter().collect();
    rows.sort_by_key(|(_, (_, tok))| std::cmp::Reverse(*tok));
    println!("  {:<40}{:>8}{:>14}", "tool", "calls", "tokens");
    for (tool, (calls, tokens)) in rows.into_iter().take(12) {
        println!("  {tool:<40}{calls:>8}{tokens:>14}");
    }
    println!(
        "\nArm a rule with `weir hook --enforce` in your hook config once the\nnumbers above justify it.\n"
    );
    Ok(())
}
