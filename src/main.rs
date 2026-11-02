use std::io::{self, Write};
use std::process::ExitCode;

use clap::Parser;
use sd_profile::app;
use sd_profile::cli::Args;
use sd_profile::error::Error;

/// Exit codes. Usage errors exit with 2, from clap.
const EXIT_FAILURE: u8 = 1;

fn main() -> ExitCode {
    let args = Args::parse();
    let mut stdout = io::stdout().lock();
    match app::run(&args, &mut stdout).and_then(|()| stdout.flush().map_err(Error::Output)) {
        Ok(()) => ExitCode::SUCCESS,
        // The reader of the output went away (`sd-profile card.img | head`):
        // nothing left to say to anyone.
        Err(Error::Output(err)) if err.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("ERROR: {err}");
            ExitCode::from(EXIT_FAILURE)
        }
    }
}
