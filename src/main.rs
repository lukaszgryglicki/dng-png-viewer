use dng_png_viewer::cli::Cli;
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = match Cli::try_parse_compat(std::env::args_os()) {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code() as u8;
            let _ = error.print();
            return ExitCode::from(code);
        }
    };
    match cli.run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
