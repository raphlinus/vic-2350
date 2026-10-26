// Copyright 2026 Raph Levien
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Run mp3.sy's decoder on the m33 emulator, checking it against the
//! mp3dec crate, which it's a port of.
//!
//! Each file is decoded frame by frame, as mp3dec's decode example does,
//! by both decoders, and each frame's sample count, frame info and samples
//! must match exactly. Also checks that r4-r11, s16-s31 and sp are
//! preserved, and reports instructions per frame.
//!
//! Usage: mp3sim [options] files.mp3...
//!   -v            print each frame
//!   --elf PATH    run an RP2350 image, such as mp3bench's, instead of
//!                 building one: its mp3_init and mp3_decode_frame
//!   --frames N    stop after N frames of each file
//!   --profile     print instructions per function
//!   --cycles      print estimated cycles per function, from instruction
//!                 counts and symbolasm --analyze's cycles per instruction,
//!                 and the lines whose flagged instructions run the most
//!   --lines PATH  with --cycles, also write every line's cycles to PATH
//!   --no-pcm      pass no output buffer, so only headers are parsed
//!   --stages PATH write the granule buffer after each stage to PATH, in
//!                 the format of mp3dec's dump feature, with a line
//!                 "n_samples" after each frame
//!
//! symbolasm is found at $SYMBOLASM, default ../../symbolasm from here.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
};

use m33::{Cpu, load_elf};

/// Where the stack, output and input go
#[derive(Clone, Copy)]
struct Layout {
    /// The stack, below this
    stack: u32,
    pcm: u32,
    info: u32,
    input: u32,
}

impl Layout {
    /// For the ELF linked here, with everything low in memory
    const LOW: Layout = Layout {
        stack: 0x10_0000,
        pcm: 0x10_0000,
        info: 0x10_0000 + 2 * mp3dec::MAX_SAMPLES_PER_FRAME as u32,
        input: 0x11_0000,
    };
    /// For an RP2350 image, with the input in flash past it
    const RP2350: Layout = Layout {
        stack: 0x2000_8000,
        pcm: 0x2000_8000,
        info: 0x2000_8000 + 2 * mp3dec::MAX_SAMPLES_PER_FRAME as u32,
        input: 0x1100_0000,
    };
}

/// After the input, filled with junk so that reading past it shows up
const SLACK: usize = 0x1000;
/// Return address that ends a call
const SENTINEL: u32 = 0xffff_fff0;
const MAX_STEPS: u64 = 100_000_000;

const LINK_SCRIPT: &str = "
SECTIONS {
    . = 0x1000;
    .text : { *(.text*) *(.rodata*) *(.ram*) *(.data*) }
    .bss : { *(.bss*) }
}
";

struct Args {
    files: Vec<PathBuf>,
    verbose: bool,
    frames: Option<usize>,
    profile: bool,
    cycles: bool,
    lines: Option<PathBuf>,
    no_pcm: bool,
    stages: Option<PathBuf>,
    elf: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        files: vec![],
        verbose: false,
        frames: None,
        profile: false,
        cycles: false,
        lines: None,
        no_pcm: false,
        stages: None,
        elf: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "-v" => args.verbose = true,
            "--profile" => args.profile = true,
            "--cycles" => args.cycles = true,
            "--no-pcm" => args.no_pcm = true,
            "--elf" => args.elf = Some(it.next().ok_or("--elf needs a value")?.into()),
            "--lines" => args.lines = Some(it.next().ok_or("--lines needs a value")?.into()),
            "--stages" => args.stages = Some(it.next().ok_or("--stages needs a value")?.into()),
            "--frames" => {
                let n = it.next().ok_or("--frames needs a value")?;
                args.frames = Some(n.parse().map_err(|e| format!("--frames: {e}"))?);
            }
            _ if a.starts_with('-') => return Err(format!("unknown option {a}")),
            _ => args.files.push(a.into()),
        }
    }
    if args.files.is_empty() {
        return Err("no input files".into());
    }
    Ok(args)
}

fn run(cmd: &mut Command) -> Result<Vec<u8>, String> {
    let out = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{cmd:?} failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(out.stdout)
}

/// Build mp3.sy into an ELF, returning its bytes.
fn build() -> Result<Vec<u8>, String> {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = std::env::temp_dir().join(format!("mp3sim-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let symbolasm = symbolasm_dir();
    let asm = run(Command::new("cargo")
        .args(["run", "-q", "--release", "--manifest-path"])
        .arg(symbolasm.join("Cargo.toml"))
        .arg("--")
        .arg(here.join("../mp3.sy")))?;
    let file = |name: &str| dir.join(name);
    std::fs::write(file("mp3.s"), asm).map_err(|e| e.to_string())?;
    std::fs::write(file("link.ld"), LINK_SCRIPT).map_err(|e| e.to_string())?;
    run(Command::new("arm-none-eabi-as")
        .args(["-mcpu=cortex-m33", "-o"])
        .arg(file("mp3.o"))
        .arg(file("mp3.s")))?;
    run(Command::new("arm-none-eabi-ld")
        .arg("-T")
        .arg(file("link.ld"))
        .args(["-e", "mp3_decode_frame", "-o"])
        .arg(file("mp3.elf"))
        .arg(file("mp3.o")))?;
    std::fs::read(file("mp3.elf")).map_err(|e| e.to_string())
}

/// symbolasm's --analyze output for mp3.sy, with exact sizes
fn analyze() -> Result<String, String> {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let out = run(Command::new("cargo")
        .args(["run", "-q", "--release", "--manifest-path"])
        .arg(symbolasm_dir().join("Cargo.toml"))
        .arg("--")
        .arg(here.join("../mp3.sy"))
        .args([
            "--analyze",
            "--size",
            "n",
            "--cols",
            "line,size,cycwhy,cyc,asm",
        ]))?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

fn symbolasm_dir() -> PathBuf {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    std::env::var("SYMBOLASM")
        .map(PathBuf::from)
        .unwrap_or_else(|_| here.join("../../symbolasm"))
}

/// Instructions run, by the function they're in
struct Profile {
    /// Function start addresses and names, sorted
    fns: Vec<(u32, String)>,
    counts: Vec<u64>,
    /// Times run and times taken (the next pc wasn't the next
    /// instruction), by pc / 2
    pcs: Vec<(u64, u64)>,
    /// The pc of the instruction being run, and the next one
    last: Option<(u32, u32)>,
}

impl Profile {
    fn new(symbols: &HashMap<String, u32>) -> Self {
        let mut fns: Vec<(u32, String)> = symbols
            .iter()
            .filter(|(name, _)| name.starts_with("mp3_"))
            .map(|(name, &addr)| (addr, name.clone()))
            .collect();
        fns.sort();
        let counts = vec![0; fns.len()];
        let end = fns.last().map_or(0, |f| f.0) as usize + 0x10000;
        Profile {
            fns,
            counts,
            pcs: vec![(0, 0); end / 2],
            last: None,
        }
    }

    fn count(&mut self, cpu: &Cpu) {
        let pc = cpu.regs.r[15];
        if let Some((last, next)) = self.last.take()
            && pc != next
        {
            self.pcs[last as usize / 2].1 += 1;
        }
        let i = self.fns.partition_point(|(addr, _)| *addr <= pc);
        if i > 0 {
            self.counts[i - 1] += 1;
        }
        if let Some(c) = self.pcs.get_mut(pc as usize / 2) {
            c.0 += 1;
            let wide = cpu.load_u16(pc) >> 11 >= 0b11101;
            self.last = Some((pc, pc + if wide { 4 } else { 2 }));
        }
    }

    /// Print estimated cycles per function and the hottest lines, given
    /// the per-instruction analysis, and write every line's to lines_path
    fn print_cycles(&self, analysis: &str, lines_path: Option<&Path>) -> Result<(), String> {
        let addrs: HashMap<&str, u32> = self
            .fns
            .iter()
            .map(|(addr, name)| (name.as_str(), addr & !1))
            .collect();
        // Cycles and lost cycles (those with a why code) by function, and
        // by line
        let mut fns: Vec<(String, u64, u64)> = vec![];
        let mut lines: HashMap<u32, (u64, u64, String)> = HashMap::new();
        let mut pc = None;
        for row in analysis.lines() {
            if let Some(rest) = row.strip_prefix("== ") {
                let name = rest.split_whitespace().next().unwrap_or("");
                pc = addrs.get(name).copied();
                fns.push((name.to_string(), 0, 0));
                continue;
            }
            let Some(addr) = pc.as_mut() else { continue };
            // line: size why cyc asm, with --cols line,size,cycwhy,cyc,asm
            let Some((line, rest)) = row.trim_start().split_once(": ") else {
                continue;
            };
            let Ok(line) = line.parse::<u32>() else {
                continue;
            };
            let size = &rest[..1];
            let Ok(size) = size.parse::<u32>() else {
                continue;
            };
            let why = rest[2..6].trim();
            let mut fields = rest[6..].split_whitespace();
            let cyc = fields.next().unwrap_or("0");
            let asm: Vec<&str> = fields.collect();
            let (not_taken, taken) = match cyc.split_once('/') {
                Some((a, b)) => (a.parse().unwrap_or(0), b.parse().unwrap_or(0)),
                None => {
                    let c = cyc.parse().unwrap_or(0);
                    (c, c)
                }
            };
            let (n, t) = self.pcs.get(*addr as usize / 2).copied().unwrap_or((0, 0));
            let c = (n - t) * not_taken + t * taken;
            let lost = if why.is_empty() { 0 } else { n };
            let f = fns.last_mut().unwrap();
            f.1 += c;
            f.2 += lost;
            let l = lines.entry(line).or_insert((0, 0, String::new()));
            l.0 += c;
            l.1 += lost;
            if !l.2.is_empty() {
                l.2.push_str("; ");
            }
            l.2.push_str(&asm.join(" "));
            if !why.is_empty() {
                l.2.push_str(&format!(" [{why}]"));
            }
            *addr += size;
        }
        let total: u64 = fns.iter().map(|f| f.1).sum();
        println!("{total} cycles in all");
        fns.sort_by_key(|f| std::cmp::Reverse(f.1));
        println!(
            "{:>12} {:>6} {:>12}  function (cycles, %, runs of flagged insns)",
            "", "", ""
        );
        for (name, c, lost) in &fns {
            if *c > 0 {
                println!(
                    "{c:12} {:5.1}% {lost:12}  {name}",
                    100.0 * *c as f64 / total as f64
                );
            }
        }
        let mut lines: Vec<_> = lines.into_iter().collect();
        lines.sort_by_key(|l| std::cmp::Reverse(l.1.1));
        println!("lines with the most runs of flagged instructions:");
        for (line, (c, lost, asm)) in lines.iter().take(40) {
            println!("{line:6} {c:12} {lost:12}  {asm}");
        }
        if let Some(path) = lines_path {
            // line, cycles, runs of flagged instructions, instructions
            lines.sort_by_key(|l| l.0);
            let text: String = lines
                .iter()
                .map(|(line, (c, lost, asm))| format!("{line} {c} {lost} {asm}\n"))
                .collect();
            std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        Ok(())
    }

    fn print(&self) {
        let total: u64 = self.counts.iter().sum();
        let mut rows: Vec<_> = self.fns.iter().zip(&self.counts).collect();
        rows.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
        for ((_, name), n) in rows {
            if *n > 0 {
                println!("{n:12} {:5.1}% {name}", 100.0 * *n as f64 / total as f64);
            }
        }
    }
}

/// Snapshots of the granule buffer as each stage returns
struct Stages {
    /// Stage names by function address
    entries: HashMap<u32, &'static str>,
    /// Calls not yet returned: return address, sp, stage, r0, r1
    pending: Vec<(u32, u32, &'static str, u32, u32)>,
    /// mp3_scratch.grbuf, the first Huffman decoder output
    grbuf: Option<u32>,
    out: String,
}

const GRANULE: u32 = 576 * 4;

impl Stages {
    fn new(symbols: &HashMap<String, u32>) -> Self {
        let mut entries = HashMap::new();
        for (f, stage) in [
            ("mp3_huffman", "huffman"),
            ("mp3_ms_band", "ms"),
            ("mp3_intensity_stereo", "i_stereo"),
            ("mp3_reorder", "reorder"),
            ("mp3_antialias", "antialias"),
            ("mp3_imdct_gr", "imdct"),
            ("mp3_change_sign", "change_sign"),
            ("mp3_dct_ii", "dct_ii"),
        ] {
            entries.insert(symbols[f], stage);
        }
        Stages {
            entries,
            pending: vec![],
            grbuf: None,
            out: String::new(),
        }
    }

    fn step(&mut self, cpu: &Cpu) {
        let pc = cpu.regs.r[15];
        let sp = cpu.regs.r[13];
        if let Some(&stage) = self.entries.get(&pc) {
            let r = &cpu.regs.r;
            self.pending.push((r[14] & !1, sp, stage, r[0], r[1]));
        }
        if let Some(&(ret, call_sp, stage, r0, r1)) = self.pending.last()
            && pc == ret
            && sp == call_sp
        {
            self.pending.pop();
            let grbuf = *self.grbuf.get_or_insert(r0);
            match stage {
                "ms" if r1 == 576 => {
                    self.dump(cpu, "ms_stereo_l", grbuf);
                    self.dump(cpu, "ms_stereo_r", grbuf + GRANULE);
                }
                "ms" => {}
                "i_stereo" => self.dump(cpu, stage, grbuf),
                "reorder" => {
                    let ch = (r0 - grbuf) / GRANULE;
                    self.dump(cpu, stage, grbuf + ch * GRANULE);
                }
                _ => self.dump(cpu, stage, r0),
            }
        }
    }

    fn dump(&mut self, cpu: &Cpu, name: &str, addr: u32) {
        self.out.push_str(name);
        for i in 0..576 {
            let x = f32::from_bits(cpu.load_u32(addr + 4 * i));
            self.out.push_str(&format!(" {x}"));
        }
        self.out.push('\n');
    }
}

/// Where things are, and what to record while running
struct Watch {
    layout: Layout,
    profile: Option<Profile>,
    stages: Option<Stages>,
    /// Function entry points, where r12 is clobbered as a linker's
    /// long branch thunk may
    entries: Vec<u32>,
}

/// Call a function, returning r0 and the number of instructions it ran.
/// Checks that the callee-saved registers survive.
fn call(cpu: &mut Cpu, addr: u32, args: &[u32], watch: &mut Watch) -> Result<(u32, u64), String> {
    for (i, arg) in args.iter().enumerate() {
        cpu.regs.r[i] = *arg;
    }
    // Poison the callee-saved registers
    for i in 4..12 {
        cpu.regs.r[i] = 0xdead_0000 + i as u32;
    }
    for i in 16..32 {
        cpu.regs.s[i] = 0x7fc0_0000 + i as u32;
    }
    let saved = cpu.regs.clone();
    cpu.regs.r[13] = watch.layout.stack;
    cpu.regs.r[14] = SENTINEL | 1;
    cpu.regs.r[15] = addr & !1;
    let mut steps = 0;
    while cpu.regs.r[15] != SENTINEL {
        if cpu.halted {
            return Err(format!("halted at pc {:#x}", cpu.regs.r[15]));
        }
        if steps == MAX_STEPS {
            return Err(format!(
                "no return after {MAX_STEPS} instructions, pc {:#x}",
                cpu.regs.r[15]
            ));
        }
        if let Some(p) = &mut watch.profile {
            p.count(cpu);
        }
        if let Some(s) = &mut watch.stages {
            s.step(cpu);
        }
        if watch.entries.binary_search(&cpu.regs.r[15]).is_ok() {
            cpu.regs.r[12] = 0xdead_000c;
        }
        cpu.step_instr();
        steps += 1;
    }
    if cpu.regs.r[13] != watch.layout.stack {
        return Err(format!("sp is {:#x} after the call", cpu.regs.r[13]));
    }
    for i in 4..12 {
        if cpu.regs.r[i] != saved.r[i] {
            return Err(format!("r{i} not preserved"));
        }
    }
    for i in 16..32 {
        if cpu.regs.s[i] != saved.s[i] {
            return Err(format!("s{i} not preserved"));
        }
    }
    Ok((cpu.regs.r[0], steps))
}

struct Stats {
    frames: usize,
    decoded: usize,
    steps: u64,
    max_steps: u64,
}

fn check_file(
    elf: &[u8],
    layout: Layout,
    path: &Path,
    args: &Args,
    stats: &mut Stats,
) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let input = layout.input as usize;
    let len = (input + bytes.len() + SLACK).max(0x2008_0000 * (layout.stack >> 29) as usize);
    let mut mem = vec![0; len];
    let symbols: HashMap<String, u32> = load_elf(elf, &mut mem);
    mem[input..][..bytes.len()].copy_from_slice(&bytes);
    mem[input + bytes.len()..][..SLACK].fill(0xff);
    let mut cpu = Cpu::new(mem);
    let sym = |name: &str| {
        symbols
            .get(name)
            .copied()
            .ok_or(format!("no symbol {name}"))
    };
    let decode = sym("mp3_decode_frame")?;
    let mut watch = Watch {
        layout,
        entries: vec![],
        profile: None,
        stages: None,
    };
    call(&mut cpu, sym("mp3_init")?, &[], &mut watch)?;
    let mut entries: Vec<u32> = symbols
        .iter()
        .filter(|(name, _)| name.starts_with("mp3_"))
        .map(|(_, &addr)| addr & !1)
        .collect();
    entries.sort();
    let mut watch = Watch {
        layout,
        entries,
        profile: (args.profile || args.cycles).then(|| Profile::new(&symbols)),
        stages: args.stages.as_ref().map(|_| Stages::new(&symbols)),
    };
    // Flash, which is read only on the RP2350
    let flash = |cpu: &Cpu| -> Vec<u8> {
        if layout.stack >> 29 == 0 {
            return vec![];
        }
        (0x1000_0000..0x1010_0000).map(|a| cpu.load_u8(a)).collect()
    };
    let before = flash(&cpu);
    let mut result = decode_file(&mut cpu, &bytes, decode, path, args, stats, &mut watch);
    if let Some(i) = before.iter().zip(flash(&cpu)).position(|(a, b)| *a != b) {
        result = result.and(Err(format!("wrote to flash at {:#x}", 0x1000_0000 + i)));
    }
    if let Some(p) = &watch.profile {
        if args.profile {
            p.print();
        }
        if args.cycles {
            p.print_cycles(&analyze()?, args.lines.as_deref())?;
        }
    }
    if let (Some(s), Some(path)) = (&watch.stages, &args.stages) {
        std::fs::write(path, &s.out).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    result
}

fn decode_file(
    cpu: &mut Cpu,
    bytes: &[u8],
    decode: u32,
    path: &Path,
    args: &Args,
    stats: &mut Stats,
    watch: &mut Watch,
) -> Result<(), String> {
    let mut decoder = mp3dec::Decoder::new();
    let mut info = mp3dec::FrameInfo::default();
    let mut pcm = [0i16; mp3dec::MAX_SAMPLES_PER_FRAME];
    let mut offset = 0;
    let mut frame = 0;
    while offset < bytes.len() && args.frames.is_none_or(|n| frame < n) {
        // Poison the output, which is only written up to n samples
        for i in 0..mp3dec::MAX_SAMPLES_PER_FRAME as u32 {
            cpu.store_u16(watch.layout.pcm + 2 * i, 0x5a5a);
        }
        let len = (bytes.len() - offset) as u32;
        let Layout {
            pcm: pcm_addr,
            info: info_addr,
            input,
            ..
        } = watch.layout;
        let result = call(
            cpu,
            decode,
            &[
                input + offset as u32,
                len,
                if args.no_pcm { 0 } else { pcm_addr },
                info_addr,
            ],
            watch,
        );
        if let Some(s) = &mut watch.stages {
            s.out.push_str("n_samples\n");
        }
        let (n, steps) = result.map_err(|e| format!("frame {frame} at {offset}: {e}"))?;
        let out = (!args.no_pcm).then_some(&mut pcm);
        let want = decoder.decode_frame(&bytes[offset..], out, &mut info);
        let field = |i: u32| cpu.load_u32(info_addr + 4 * i) as usize;
        let got_info = [field(0), field(1), field(2), field(3), field(4), field(5)];
        let want_info = [
            info.frame_bytes,
            info.frame_offset,
            info.channels,
            info.hz,
            info.layer,
            info.bitrate_kbps,
        ];
        let at = format!("{} frame {frame} at {offset}", path.display());
        if args.verbose {
            println!("{at}: {n} samples, info {got_info:?}, {steps} instructions");
        }
        if n as usize != want {
            return Err(format!("{at}: {n} samples, want {want}"));
        }
        // Both leave the fields other than frame_bytes as they were when
        // no frame is found
        if got_info != want_info {
            return Err(format!("{at}: info {got_info:?}, want {want_info:?}"));
        }
        let count = if args.no_pcm { 0 } else { want * info.channels };
        for (i, &w) in pcm[..count].iter().enumerate() {
            let g = cpu.load_u16(pcm_addr + 2 * i as u32) as i16;
            if g != w {
                let ch = i % info.channels;
                return Err(format!(
                    "{at}: sample {} channel {ch} is {g}, want {w}",
                    i / info.channels
                ));
            }
        }
        stats.frames += 1;
        if want != 0 {
            stats.decoded += 1;
            stats.steps += steps;
            stats.max_steps = stats.max_steps.max(steps);
        }
        if info.frame_bytes == 0 {
            break;
        }
        offset += info.frame_bytes;
        frame += 1;
    }
    Ok(())
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };
    let (elf, layout) = match &args.elf {
        Some(path) => match std::fs::read(path) {
            Ok(elf) => (Ok(elf), Layout::RP2350),
            Err(e) => (Err(format!("{}: {e}", path.display())), Layout::RP2350),
        },
        None => (build(), Layout::LOW),
    };
    let elf = match elf {
        Ok(elf) => elf,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let mut failed = 0;
    for path in &args.files {
        let mut stats = Stats {
            frames: 0,
            decoded: 0,
            steps: 0,
            max_steps: 0,
        };
        match check_file(&elf, layout, path, &args, &mut stats) {
            Ok(()) => {
                let avg = stats.steps.checked_div(stats.decoded as u64).unwrap_or(0);
                println!(
                    "ok {}: {} frames, {} decoded, {avg} instructions per frame (max {})",
                    path.display(),
                    stats.frames,
                    stats.decoded,
                    stats.max_steps
                );
            }
            Err(e) => {
                println!("FAIL {e}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        println!("{failed} of {} files failed", args.files.len());
        std::process::exit(1);
    }
}
