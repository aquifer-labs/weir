// SPDX-License-Identifier: Apache-2.0
//! Weir — measure and bound what flows into an agent's context.
//!
//! `weir scan` reads local Claude Code and Codex session logs and reports where
//! context pressure actually comes from. Pressure is the metric that matters:
//! a tool result costs its own tokens multiplied by every later model turn that
//! re-reads it, so a small output early in a long session outweighs a large one
//! at the end.

mod config;
mod doctor;
mod gate;
mod hook;
mod init;
mod policy;
mod scan;
mod shadow;
mod shape;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "weir", version, about = "Measure and bound agent context", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Measure context pressure in local agent session logs
    Scan(scan::ScanArgs),
    /// Install Weir's hooks into the harnesses you already use
    Init(init::InitArgs),
    /// Hook entry point (reads a hook payload on stdin)
    Hook(hook::HookArgs),
    /// Report what shadow mode would have changed
    Shadow(shadow::ShadowArgs),
    /// Check that the binary, the plugin and the hooks are in step
    Doctor(doctor::DoctorArgs),
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Scan(args) => scan::run(args),
        Command::Init(args) => init::run(args),
        Command::Hook(args) => hook::run(args),
        Command::Shadow(args) => shadow::run(args),
        Command::Doctor(args) => doctor::run(args),
    }
}
