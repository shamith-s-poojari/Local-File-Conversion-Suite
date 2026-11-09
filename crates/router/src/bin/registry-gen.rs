//! Builds `registry.generated.json` from `adapters/*/manifest.yaml` (spec section 8.2).
//!
//! ```text
//! registry-gen [--root <repo>] [--out <file>] [--include-dev] [--check]
//! ```
//!
//! Exits non-zero, listing every problem, on duplicate IDs, unknown warning
//! codes, unapproved engines, or invalid manifests or parameter schemas.
//! `--check` validates without writing.

use std::path::PathBuf;
use std::process::ExitCode;

use converter_router::{build_registry, Catalog};

fn main() -> ExitCode {
    let mut root = PathBuf::from(".");
    let mut out: Option<PathBuf> = None;
    let mut include_dev = false;
    let mut check = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = args.next().map(PathBuf::from).unwrap_or(root),
            "--out" => out = args.next().map(PathBuf::from),
            "--include-dev" => include_dev = true,
            "--check" => check = true,
            "-h" | "--help" => {
                println!(
                    "usage: registry-gen [--root <repo>] [--out <file>] [--include-dev] [--check]"
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("registry-gen: unknown argument {other:?}");
                return ExitCode::from(64);
            }
        }
    }

    let result = Catalog::load(&root)
        .and_then(|catalog| build_registry(&root.join("adapters"), &catalog, include_dev));
    let registry = match result {
        Ok(r) => r,
        Err(problems) => {
            eprintln!("registry-gen: {} problem(s):", problems.len());
            for p in problems {
                eprintln!("  {p}");
            }
            return ExitCode::FAILURE;
        }
    };

    if check {
        println!("registry-gen: {} adapter(s) OK", registry.adapters.len());
        return ExitCode::SUCCESS;
    }
    let out = out.unwrap_or_else(|| root.join("registry.generated.json"));
    if let Err(e) = std::fs::write(&out, registry.to_json()) {
        eprintln!("registry-gen: {}: {e}", out.display());
        return ExitCode::FAILURE;
    }
    println!(
        "registry-gen: wrote {} adapter(s) to {}",
        registry.adapters.len(),
        out.display()
    );
    ExitCode::SUCCESS
}
