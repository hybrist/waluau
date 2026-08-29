//! The `waluau fmt` subcommand: format `.walu` files in place, check
//! formatting in CI, or format stdin to stdout.
//!
//! Usage:
//! ```text
//! waluau fmt [FLAGS] [PATH ...]
//!   PATH        a .walu file or a directory searched recursively; omitted or
//!               `-` reads stdin and writes stdout
//!   --check     do not write; exit non-zero if any file is not already
//!               formatted, listing the offenders
//!   --exclude P skip a file or directory (repeatable)
//!   --width N   target line width (default 100)
//!   --indent N  spaces per indent level (default 4)
//! ```

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use waluau_diagnostics::Diagnostic;
use waluau_fmt::{FormatConfig, format_source};

struct FmtOptions {
    paths: Vec<PathBuf>,
    excludes: Vec<PathBuf>,
    check: bool,
    config: FormatConfig,
}

/// Run the `fmt` subcommand. `args` are the tokens after `fmt`.
pub fn run_fmt<I>(args: I) -> Result<(), Vec<Diagnostic>>
where
    I: IntoIterator<Item = OsString>,
{
    let options = parse_fmt_args(args).map_err(|e| vec![e])?;

    let excludes = options
        .excludes
        .iter()
        .map(|path| {
            std::fs::canonicalize(path)
                .map_err(|e| Diagnostic::new(format!("exclude path {}: {e}", path.display())))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| vec![e])?;

    // No paths: format stdin to stdout (or check it).
    if options.paths.is_empty() {
        return run_stdin(&options).map_err(|e| vec![e]);
    }

    let mut files = Vec::new();
    for path in &options.paths {
        collect_walu_files(path, &excludes, &mut files).map_err(|e| vec![e])?;
    }
    files.sort();
    files.dedup();

    let mut errors = Vec::new();
    let mut unformatted = Vec::new();
    for file in &files {
        match format_file(file, &options) {
            Ok(true) => unformatted.push(file.clone()),
            Ok(false) => {}
            Err(e) => errors.push(e),
        }
    }

    if options.check && !unformatted.is_empty() {
        for file in &unformatted {
            errors.push(Diagnostic::new(format!(
                "not formatted: {}",
                file.display()
            )));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Format one file. Returns whether the file's content differs from its
/// formatted form (i.e. it needs formatting). In write mode a differing file is
/// rewritten as a side effect.
fn format_file(path: &Path, options: &FmtOptions) -> Result<bool, Diagnostic> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| Diagnostic::new(format!("read {}: {e}", path.display())))?;
    let formatted = format_source(&source, &options.config)
        .map_err(|e| Diagnostic::new(format!("{}: {}", path.display(), e.render())))?;
    let differs = formatted != source;
    if differs && !options.check {
        std::fs::write(path, &formatted)
            .map_err(|e| Diagnostic::new(format!("write {}: {e}", path.display())))?;
    }
    Ok(differs)
}

fn run_stdin(options: &FmtOptions) -> Result<(), Diagnostic> {
    let mut source = String::new();
    std::io::stdin()
        .read_to_string(&mut source)
        .map_err(|e| Diagnostic::new(format!("read stdin: {e}")))?;
    let formatted = format_source(&source, &options.config)
        .map_err(|e| Diagnostic::new(format!("<stdin>: {}", e.render())))?;
    if options.check {
        if formatted != source {
            return Err(Diagnostic::new("not formatted: <stdin>"));
        }
        return Ok(());
    }
    std::io::stdout()
        .write_all(formatted.as_bytes())
        .map_err(|e| Diagnostic::new(format!("write stdout: {e}")))
}

/// Add `path` to `out` if it is a `.walu` file, or recurse if it is a
/// directory.
fn collect_walu_files(
    path: &Path,
    excludes: &[PathBuf],
    out: &mut Vec<PathBuf>,
) -> Result<(), Diagnostic> {
    let meta =
        std::fs::metadata(path).map_err(|e| Diagnostic::new(format!("{}: {e}", path.display())))?;
    if meta.is_dir()
        && matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some(".git" | "node_modules" | "target")
        )
    {
        return Ok(());
    }
    let canonical = std::fs::canonicalize(path)
        .map_err(|e| Diagnostic::new(format!("{}: {e}", path.display())))?;
    if excludes
        .iter()
        .any(|excluded| canonical == *excluded || canonical.starts_with(excluded))
    {
        return Ok(());
    }
    if meta.is_dir() {
        let entries = std::fs::read_dir(path)
            .map_err(|e| Diagnostic::new(format!("read dir {}: {e}", path.display())))?;
        for entry in entries {
            let entry =
                entry.map_err(|e| Diagnostic::new(format!("read dir {}: {e}", path.display())))?;
            collect_walu_files(&entry.path(), excludes, out)?;
        }
    } else if path.extension().is_some_and(|e| e == "walu") {
        out.push(path.to_path_buf());
    }
    Ok(())
}

fn parse_fmt_args<I>(args: I) -> Result<FmtOptions, Diagnostic>
where
    I: IntoIterator<Item = OsString>,
{
    let mut paths = Vec::new();
    let mut excludes = Vec::new();
    let mut check = false;
    let mut config = FormatConfig::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        let arg = arg.to_string_lossy().into_owned();
        match arg.as_str() {
            "--check" => check = true,
            "-" => {} // explicit stdin: leave paths empty
            "--exclude" => excludes
                .push(PathBuf::from(iter.next().ok_or_else(|| {
                    Diagnostic::new("--exclude requires a path")
                })?)),
            flag if flag.starts_with("--exclude=") => {
                excludes.push(PathBuf::from(&flag["--exclude=".len()..]));
            }
            "--width" => config.line_width = parse_usize_flag(&mut iter, "--width")?,
            "--indent" => config.indent_width = parse_usize_flag(&mut iter, "--indent")?,
            flag if flag.starts_with("--width=") => {
                config.line_width = parse_usize(&flag["--width=".len()..], "--width")?;
            }
            flag if flag.starts_with("--indent=") => {
                config.indent_width = parse_usize(&flag["--indent=".len()..], "--indent")?;
            }
            flag if flag.starts_with('-') && flag != "-" => {
                return Err(Diagnostic::new(format!("unknown fmt flag '{flag}'")));
            }
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    Ok(FmtOptions {
        paths,
        excludes,
        check,
        config,
    })
}

fn parse_usize_flag(
    iter: &mut impl Iterator<Item = OsString>,
    flag: &str,
) -> Result<usize, Diagnostic> {
    let value = iter
        .next()
        .ok_or_else(|| Diagnostic::new(format!("{flag} requires a value")))?;
    parse_usize(&value.to_string_lossy(), flag)
}

fn parse_usize(value: &str, flag: &str) -> Result<usize, Diagnostic> {
    value
        .parse::<usize>()
        .ok()
        .filter(|&n| n > 0)
        .ok_or_else(|| Diagnostic::new(format!("{flag} expects a positive integer, got '{value}'")))
}
