// Copyright 2026 Raph Levien
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Compile ../mp3.sy with symbolasm, assemble it with GNU as, and link it
// as a static library. Its functions' sections become .data ones, which
// cortex-m-rt copies to RAM. Also copies the input file to OUT_DIR, and
// decodes it with mp3dec here on the host, for the hash of its samples
// that the board checks against.

use std::{env, path::PathBuf, process::Command};

fn run(cmd: &mut Command) {
    let status = cmd.status().unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
    assert!(status.success(), "{cmd:?} failed");
}

/// FNV-1a over the samples of each frame; must match main.rs
fn hash(h: u32, samples: &[i16]) -> u32 {
    samples.iter().fold(h, |h, s| {
        let h = (h ^ (*s as u16 as u32 & 0xff)).wrapping_mul(0x0100_0193);
        (h ^ (*s as u16 as u32 >> 8)).wrapping_mul(0x0100_0193)
    })
}

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let here = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let symbolasm = env::var("SYMBOLASM")
        .map(PathBuf::from)
        .unwrap_or_else(|_| here.join("../../symbolasm"));
    let mp3_sy = here.join("../mp3.sy");
    let output = Command::new("cargo")
        .args(["run", "-q", "--release", "--"])
        .arg(&mp3_sy)
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
    let asm = String::from_utf8(output.stdout)
        .unwrap()
        .replace(".section .ram.", ".section .data.")
        // All of it, as mp3dec's code is (see memory.x); the tables, in
        // plain .text, stay in flash, as mp3dec's do
        .replace(".section .text.mp3_", ".section .data.mp3_");
    let asm_path = out.join("mp3.s");
    std::fs::write(&asm_path, asm).unwrap();
    let obj = out.join("mp3.o");
    run(Command::new("arm-none-eabi-as")
        .args(["-mcpu=cortex-m33", "-o"])
        .arg(&obj)
        .arg(&asm_path));
    let lib = out.join("libmp3.a");
    let _ = std::fs::remove_file(&lib);
    run(Command::new("arm-none-eabi-ar")
        .arg("rcs")
        .arg(&lib)
        .arg(&obj));
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=mp3");

    let input = PathBuf::from(env::var("MP3").expect("set MP3 to the mp3 file to decode"));
    let bytes = std::fs::read(&input).unwrap_or_else(|e| panic!("{}: {e}", input.display()));
    std::fs::write(out.join("input.mp3"), &bytes).unwrap();
    let mut decoder = mp3dec::Decoder::new();
    let mut info = mp3dec::FrameInfo::default();
    let mut pcm = [0; mp3dec::MAX_SAMPLES_PER_FRAME];
    let mut h = 0x811c_9dc5;
    let mut offset = 0;
    while offset < bytes.len() {
        let n = decoder.decode_frame(&bytes[offset..], Some(&mut pcm), &mut info);
        h = hash(h, &pcm[..n * info.channels]);
        if info.frame_bytes == 0 {
            break;
        }
        offset += info.frame_bytes;
    }
    std::fs::write(
        out.join("expected.rs"),
        format!("/// The hash of all the samples, from mp3dec on the host\nconst EXPECTED: u32 = {h:#010x};\n"),
    )
    .unwrap();

    println!("cargo:rerun-if-env-changed=MP3");
    println!("cargo:rerun-if-changed={}", input.display());
    println!("cargo:rerun-if-changed=../mp3.sy");
    println!("cargo:rerun-if-changed=../mp3_tables.sy");
    println!("cargo:rerun-if-changed=build.rs");
}
