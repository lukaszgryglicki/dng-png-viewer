use dng_png_viewer::cli::Cli;
use std::process::ExitCode;

#[cfg(all(target_os = "linux", target_env = "musl"))]
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    #[cfg(target_os = "freebsd")]
    // Before any threads start: never let SDL detach the physical console keyboard.
    unsafe {
        std::env::set_var("SDL_INPUT_FREEBSD_KEEP_KBD", "1");
    }
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
