use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process;

use clap::Parser;
use patcherd::search::{self, Pattern};
use serde::Deserialize;

#[derive(Parser)]
#[command(about = "Binary patcher - find and replace byte sequences in files")]
struct Cli {
    /// Path to the input binary
    #[arg(long)]
    input: String,

    /// Path to write the patched binary
    #[arg(long)]
    output: String,

    /// Hex byte sequence to find (repeatable, supports ?? wildcards)
    #[arg(long)]
    find: Vec<String>,

    /// Hex byte sequence to replace with (repeatable, must pair with --find)
    #[arg(long)]
    replace: Vec<String>,

    /// Print what would change without writing the file
    #[arg(long, default_value_t = false)]
    dry_run: bool,
}

#[derive(Deserialize)]
struct PatchSpec {
    find: String,
    replace: String,
}

/// Decode a hex string into a pattern, supporting `??` as wildcard.
fn decode_hex_pattern(s: &str) -> Result<Vec<Pattern>, String> {
    let cleaned = s
        .replace([' ', ','], "")
        .replace("0x", "")
        .replace("0X", "");

    if !cleaned.len().is_multiple_of(2) {
        return Err(format!("hex string '{}' has odd length", s));
    }

    let mut patterns = Vec::with_capacity(cleaned.len() / 2);
    for i in (0..cleaned.len()).step_by(2) {
        let pair = &cleaned[i..i + 2];
        if pair == "??" {
            patterns.push(Pattern::Wildcard);
        } else {
            let byte = u8::from_str_radix(pair, 16)
                .map_err(|e| format!("invalid hex '{}': {}", pair, e))?;
            patterns.push(Pattern::Byte(byte));
        }
    }
    Ok(patterns)
}

/// Decode a hex string into raw bytes (no wildcards allowed).
fn decode_hex(s: &str) -> Result<Vec<u8>, String> {
    let cleaned = s
        .replace([' ', ','], "")
        .replace("0x", "")
        .replace("0X", "");
    hex::decode(&cleaned).map_err(|e| format!("hex decode error for '{}': {}", s, e))
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();

    if cli.find.len() != cli.replace.len() {
        return Err("--find and --replace flags must be provided in pairs".into());
    }

    let mut specs: Vec<PatchSpec> = cli
        .find
        .into_iter()
        .zip(cli.replace)
        .map(|(find, replace)| PatchSpec { find, replace })
        .collect();

    if let Ok(env_rules) = env::var("PATCH_RULES") {
        let env_specs: Vec<PatchSpec> = serde_json::from_str(&env_rules)
            .map_err(|e| format!("failed to parse PATCH_RULES: {}", e))?;
        specs.extend(env_specs);
    }

    if specs.is_empty() {
        return Err(
            "no patch specs provided (use --find/--replace flags or PATCH_RULES env var)".into(),
        );
    }

    let mut data =
        fs::read(&cli.input).map_err(|e| format!("failed to read '{}': {}", cli.input, e))?;

    for spec in &specs {
        let find_pattern = decode_hex_pattern(&spec.find)?;
        let replace_bytes = decode_hex(&spec.replace)?;

        if find_pattern.len() != replace_bytes.len() {
            return Err(format!(
                "find ({} bytes) and replace ({} bytes) must be the same length",
                find_pattern.len(),
                replace_bytes.len()
            ));
        }

        // Single search pass: get positions, derive count, reuse for replacement.
        let positions = search::find_all(&data, &find_pattern);
        let count = positions.len();

        if count == 0 {
            eprintln!("warning: pattern {} not found in input", spec.find);
        } else {
            eprintln!(
                "info: found {} occurrence(s) of pattern {}",
                count, spec.find
            );
        }

        if cli.dry_run {
            eprintln!(
                "dry-run: would replace {} occurrence(s) of {} with {}",
                count, spec.find, spec.replace
            );
            continue;
        }

        data = search::replace_at_positions(&data, &positions, &find_pattern, &replace_bytes);
    }

    if cli.dry_run {
        return Ok(());
    }

    fs::write(&cli.output, &data)
        .map_err(|e| format!("failed to write '{}': {}", cli.output, e))?;
    fs::set_permissions(&cli.output, fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("failed to set permissions on '{}': {}", cli.output, e))?;

    eprintln!("patched binary written to {}", cli.output);
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {}", e);
        process::exit(1);
    }
}
