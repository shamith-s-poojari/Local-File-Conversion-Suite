//! `converter-sandbox-run`: see [`converter_sandbox::runner`].

fn main() -> std::process::ExitCode {
    converter_sandbox::runner::main_with(std::env::args().skip(1).collect())
}
