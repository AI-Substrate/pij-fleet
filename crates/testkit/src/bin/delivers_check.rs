//! The PR body's `Delivers:` line, checked before the merge (plan 209, F65).
//!
//! Reads a PR body on stdin and exits 0 when its `Delivers:` line is `none` or
//! full ddocs addresses, exactly as `pij delivered` will read it after the
//! merge (`pij_core::claims::delivers`); otherwise prints why and exits 1.
//! `.github/workflows/delivers.yml` runs it on every pull request.
use std::io::Read;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut body = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut body) {
        eprintln!("pij-delivers-check: could not read the PR body: {error}");
        return ExitCode::from(2);
    }
    match pij_core::claims::delivers(&body) {
        Ok(addresses) if addresses.is_empty() => {
            println!("Delivers: none");
            ExitCode::SUCCESS
        }
        Ok(addresses) => {
            println!("Delivers: {} address(es)", addresses.len());
            for address in addresses {
                println!("  {address}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("pij-delivers-check: {error}");
            ExitCode::FAILURE
        }
    }
}
