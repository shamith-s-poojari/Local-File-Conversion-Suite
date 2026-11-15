//! `lfcs`: the converter from a terminal. It reuses `converter-app-core`, so
//! files take the same router, per-job sandbox and output checks as the app.
//!
//! Exit codes: 0 ok, 1 a job failed, 2 usage error, 3 sandbox unavailable.

mod args;
mod commands;
mod files;

use std::process::ExitCode;

const RUNNER_FLAG: &str = "--sandbox-run";

const USAGE: &str = "\
lfcs: convert files locally, in the same sandbox as the converter app

USAGE
  lfcs doctor
  lfcs inspect FILE...
  lfcs targets FILE...
  lfcs convert FILE|DIR... --to FORMAT [--out DIR] [--set KEY=VALUE]... [--params JSON] [--keep-folders]
  lfcs run TOOL FILE|DIR... [--set KEY=VALUE]... [--params JSON] [--out PATH] [--force]

COMMANDS
  doctor    Run the sandbox self-test and list the engines found.
  inspect   Show format, size, and what a file reveals or could do.
  targets   Show the conversions and tools available for these files.
  convert   Convert every file to FORMAT, saving into --out (default: here).
  run       Run a tool (such as pdf-merge) on the files.

OPTIONS
  --to FORMAT       Target format for convert (see `lfcs targets`)
  -o, --out PATH    Where to save. A folder; for `run` with one result it may
                    be a file name (an existing file needs --force)
  --set KEY=VALUE   A parameter; VALUE is JSON when it parses (80, true, [1,2]),
                    else text. Repeatable; wins over --params
  --params JSON     Parameters as a JSON object
  --keep-folders    Save results in the folder layout of any folder given
  --force           Allow replacing an existing --out file
  --json            Machine-readable output on stdout
  -q, --quiet       No progress on stderr
  --resources DIR   Folder holding adapters/, engines/ and the Python runtime
                    (default: the repository around this program)
  --data-dir DIR    Working folder for jobs (default: the app's data folder)
  -h, --help        This text
  --version         Version

Nothing is ever overwritten: results get a numeric suffix when a name is taken.
Exit codes: 0 ok, 1 a job failed, 2 usage error, 3 sandbox unavailable.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    // This program is its own sandbox runner, like the desktop app.
    if args.get(1).map(String::as_str) == Some(RUNNER_FLAG) {
        return converter_sandbox::runner::main_with(args[2..].to_vec());
    }
    let opts = match args::parse(args.into_iter().skip(1)) {
        Ok(o) => o,
        Err(e) => return usage(&e),
    };
    if opts.help {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if opts.version {
        println!("lfcs {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    match commands::dispatch(&opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if !e.message.is_empty() {
                eprintln!("lfcs: {}", e.message);
            }
            if e.code == 2 {
                eprintln!("try `lfcs --help`");
            }
            ExitCode::from(e.code)
        }
    }
}

fn usage(message: &str) -> ExitCode {
    eprintln!("lfcs: {message}");
    eprintln!("try `lfcs --help`");
    ExitCode::from(2)
}
