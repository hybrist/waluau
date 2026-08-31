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

#[test]
fn excludes_files_and_directories_from_recursive_formatting() {
    let dir = std::env::temp_dir().join(format!("waluau-fmt-x-{}", std::process::id()));
    let excluded_dir = dir.join("vendor");
    std::fs::create_dir_all(&excluded_dir).unwrap();
    let included = dir.join("included.walu");
    let excluded_file = dir.join("invalid.walu");
    let excluded_nested = excluded_dir.join("vendored.walu");
    std::fs::write(&included, "local  value=1\n").unwrap();
    std::fs::write(&excluded_file, "local value =\n").unwrap();
    std::fs::write(&excluded_nested, "local vendored =\n").unwrap();

    run_with_args(args(&[
        "fmt",
        "--exclude",
        excluded_file.to_str().unwrap(),
        "--exclude",
        excluded_dir.to_str().unwrap(),
        dir.to_str().unwrap(),
    ]))
    .expect("excluded invalid sources do not prevent formatting");

    assert_eq!(
        std::fs::read_to_string(&included).unwrap(),
        "local value = 1\n"
    );
    assert_eq!(
        std::fs::read_to_string(&excluded_file).unwrap(),
        "local value =\n"
    );
    assert_eq!(
        std::fs::read_to_string(&excluded_nested).unwrap(),
        "local vendored =\n"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn recursive_formatting_skips_dependency_and_build_directories() {
    let dir = std::env::temp_dir().join(format!("waluau-fmt-s-{}", std::process::id()));
    let dependency_dir = dir.join("node_modules");
    let build_dir = dir.join("target");
    let cache_dir = dir.join(".waluau").join("0cd1f05e8502");
    std::fs::create_dir_all(&dependency_dir).unwrap();
    std::fs::create_dir_all(&build_dir).unwrap();
    std::fs::create_dir_all(&cache_dir).unwrap();
    std::fs::write(dir.join("source.walu"), "local  source=1\n").unwrap();
    std::fs::write(
        dependency_dir.join("dependency.walu"),
        "local dependency =\n",
    )
    .unwrap();
    std::fs::write(build_dir.join("generated.walu"), "local generated =\n").unwrap();
    std::fs::write(cache_dir.join("cached.walu"), "local cached =\n").unwrap();

    run_with_args(args(&["fmt", dir.to_str().unwrap()]))
        .expect("ignored directories are not parsed or formatted");

    assert_eq!(
        std::fs::read_to_string(dir.join("source.walu")).unwrap(),
        "local source = 1\n"
    );
    assert_eq!(
        std::fs::read_to_string(dependency_dir.join("dependency.walu")).unwrap(),
        "local dependency =\n"
    );
    assert_eq!(
        std::fs::read_to_string(build_dir.join("generated.walu")).unwrap(),
        "local generated =\n"
    );
    assert_eq!(
        std::fs::read_to_string(cache_dir.join("cached.walu")).unwrap(),
        "local cached =\n"
    );

    std::fs::remove_dir_all(&dir).ok();
}
