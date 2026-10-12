use std::process::ExitCode;

fn main() -> ExitCode {
    #[cfg(target_os = "macos")]
    {
        if let Err(e) = rustpaste::ui::run() {
            eprintln!("rustpaste: {e}");
            return ExitCode::FAILURE;
        }
        ExitCode::SUCCESS
    }
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("RustPaste only runs on macOS.");
        ExitCode::FAILURE
    }
}
