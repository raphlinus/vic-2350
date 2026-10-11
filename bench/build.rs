// Compile ../fill.sy with symbolasm, assemble it with GNU as, and link it
// as a static library, the same objects vic links.

use std::{env, path::PathBuf, process::Command};

fn run(cmd: &mut Command) {
    let status = cmd.status().unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
    assert!(status.success(), "{cmd:?} failed");
}

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let symbolasm = env::var("SYMBOLASM")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env::var("HOME").unwrap()).join("garden/symbolasm"));
    let asm = out.join("fill.s");
    let fill_sy = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("../fill.sy");
    let output = Command::new("cargo")
        .args(["run", "-q", "--"])
        .arg(&fill_sy)
        // Run from symbolasm's directory, so this crate's .cargo/config.toml
        // and cross-compilation settings don't apply.
        .current_dir(&symbolasm)
        .env_remove("CARGO_BUILD_TARGET")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .output()
        .expect("running symbolasm");
    if !output.status.success() {
        panic!(
            "symbolasm failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    std::fs::write(&asm, &output.stdout).unwrap();
    let obj = out.join("fill.o");
    run(Command::new("arm-none-eabi-as")
        .args(["-mcpu=cortex-m33", "-o"])
        .arg(&obj)
        .arg(&asm));
    let lib = out.join("libfill.a");
    let _ = std::fs::remove_file(&lib);
    run(Command::new("arm-none-eabi-ar").arg("rcs").arg(&lib).arg(&obj));
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=fill");
    println!("cargo:rerun-if-changed=../fill.sy");
    println!("cargo:rerun-if-changed=build.rs");
}
