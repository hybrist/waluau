use std::process::ExitCode;

fn main() -> ExitCode {
    match waluau_driver::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(errors) => {
            for error in &errors {
                eprintln!("{}", error.render());
            }
            ExitCode::FAILURE
        }
    }
}
