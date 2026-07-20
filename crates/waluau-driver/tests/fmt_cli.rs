//! Integration coverage for the `waluau fmt` subcommand.

use std::ffi::OsString;

use waluau_driver::run_with_args;

fn args(parts: &[&str]) -> Vec<OsString> {
    parts.iter().map(OsString::from).collect()
}

#[test]
fn check_fails_on_unformatted_and_does_not_write() {
    let dir = std::env::temp_dir().join(format!("waluau-fmt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("unformatted.walu");
    let original = "function f(a:i32):i32\nreturn a\nend\n";
    std::fs::write(&file, original).unwrap();

    let result = run_with_args(args(&["fmt", "--check", file.to_str().unwrap()]));
    assert!(result.is_err(), "check should fail on unformatted input");
    // --check must not modify the file.
    assert_eq!(std::fs::read_to_string(&file).unwrap(), original);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn write_mode_formats_in_place_and_is_then_clean() {
    let dir = std::env::temp_dir().join(format!("waluau-fmt-w-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("messy.walu");
    std::fs::write(&file, "local   x=1\nlocal y   = 2\n").unwrap();

    run_with_args(args(&["fmt", file.to_str().unwrap()])).expect("write-mode fmt succeeds");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "local x = 1\nlocal y = 2\n"
    );
    // Now --check passes.
    run_with_args(args(&["fmt", "--check", file.to_str().unwrap()]))
        .expect("formatted file passes --check");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn formats_directory_recursively() {
    let dir = std::env::temp_dir().join(format!("waluau-fmt-d-{}", std::process::id()));
    let sub = dir.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(dir.join("a.walu"), "local  a=1\n").unwrap();
    std::fs::write(sub.join("b.walu"), "local  b=2\n").unwrap();
    // A non-.walu file must be ignored.
    std::fs::write(dir.join("readme.txt"), "not code").unwrap();

    run_with_args(args(&["fmt", dir.to_str().unwrap()])).expect("dir fmt succeeds");
    assert_eq!(
        std::fs::read_to_string(dir.join("a.walu")).unwrap(),
        "local a = 1\n"
    );
    assert_eq!(
        std::fs::read_to_string(sub.join("b.walu")).unwrap(),
        "local b = 2\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("readme.txt")).unwrap(),
        "not code"
    );

    std::fs::remove_dir_all(&dir).ok();
}
