//! Cross-process byte-determinism of compiled artifacts.
//!
//! Every `HashMap` in the compiler gets a fresh random hash seed per
//! process, so iteration-order leaks into the artifact only show up when
//! the same input is compiled by two separate compiler processes — an
//! in-process double-build (like the incremental tests) cannot catch them.
//! This spawns the real CLI twice per fixture and requires identical bytes.

use std::path::Path;
use std::process::Command;

/// Exercises the constructs whose lowering historically leaked map
/// iteration order: if/else merges (phi creation), loops with `break`
/// (exit phis), for-in over records, closures, and tagged unions.
const FIXTURE: &str = r##"
type Goods = Upgrade({ kind: i32 }) | Spell({ kind: i32 })

function mix(color: string, seed: i32): f64
    local red: f64 = 0.0
    local green: f64 = 0.0
    local blue: f64 = 0.0
    local alpha: f64 = 1.0
    if #color == 7 or #color == 9 then
        red = 0.25
        green = 0.5
        blue = 0.75
        if #color == 9 then alpha = 0.125 end
    end
    local total: f64 = red + green + blue + alpha
    local index: i32 = 0
    while index < seed do
        total = total + red
        green = green + blue
        if total > 100.0 then
            break
        end
        index = index + 1
    end
    local sample = { red = red, green = green, blue = blue, alpha = alpha }
    for _, channel in pairs(sample) do
        total = total + channel
    end

    local scale = function(value: f64): f64
        return value * total + green
    end
    local goods: Goods = Spell({ kind = seed })
    if goods is Spell then
        total = total + scale(2.0)
    end
    return total
end

local acc: f64 = 0.0
for step = 1, 5 do
    acc = acc + mix("#80ff40", step::i32)
end
"##;

fn compile(binary: &Path, entry: &Path, out: &Path) -> Vec<u8> {
    let status = Command::new(binary)
        .arg(entry)
        .arg("-o")
        .arg(out)
        .status()
        .expect("compiler should spawn");
    assert!(status.success(), "fixture should compile");
    std::fs::read(out).expect("artifact should exist")
}

#[test]
fn separate_compiler_processes_produce_identical_bytes() {
    let binary = Path::new(env!("CARGO_BIN_EXE_waluau-cli"));
    let dir = std::env::temp_dir().join(format!("waluau-determinism-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tempdir");
    let entry = dir.join("main.walu");
    std::fs::write(&entry, FIXTURE).expect("fixture should write");

    let first = compile(binary, &entry, &dir.join("first.wasm"));
    for round in 0..4 {
        let again = compile(binary, &entry, &dir.join(format!("again{round}.wasm")));
        assert_eq!(
            first, again,
            "artifact bytes must not depend on the process's hash seed (round {round})"
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}
