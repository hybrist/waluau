use std::process::ExitCode;

fn main() -> ExitCode {
    match waluau_driver::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
