use std::ffi::OsString;
use std::io::{BufReader, BufWriter};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let result = if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        println!("{}", waluau_driver::CLI_HELP);
        Ok(())
    } else if args.len() == 1 && args[0] == "--server" {
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        waluau_driver::run_server(BufReader::new(stdin.lock()), BufWriter::new(stdout.lock()))
            .map_err(|error| vec![error])
    } else {
        waluau_driver::run_with_args(args)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(errors) => {
            for error in &errors {
                eprintln!("{}", error.render());
            }
            ExitCode::FAILURE
        }
    }
}
