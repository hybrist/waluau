//! Stdio transport for the Waluau language server: Content-Length framed
//! JSON-RPC on stdin/stdout, delegating every message to the transport-
//! agnostic [`waluau_lsp::LspServer`] core.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::ExitCode;

use waluau_lsp::LspServer;

fn main() -> ExitCode {
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    let mut server = LspServer::new();

    loop {
        let Some(message) = read_frame(&mut reader) else {
            // Client hung up without `exit`.
            return ExitCode::FAILURE;
        };
        for outgoing in server.handle_message(&message) {
            if write_frame(&mut writer, &outgoing).is_err() {
                return ExitCode::FAILURE;
            }
        }
        if server.exit_requested() {
            return if server.clean_exit() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };
        }
    }
}

fn read_frame(reader: &mut impl BufRead) -> Option<String> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = value.trim().parse().ok();
        }
    }
    let length = content_length?;
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).ok()?;
    String::from_utf8(body).ok()
}

fn write_frame(writer: &mut impl Write, message: &str) -> std::io::Result<()> {
    write!(writer, "Content-Length: {}\r\n\r\n{message}", message.len())?;
    writer.flush()
}
