mod ast;
mod check;
mod codegen;
mod error;
mod fix;
mod interp;
mod lexer;
mod loader;
mod parser;

use error::{json_str, Error, Pos, Sources};
use std::path::{Path, PathBuf};
use std::process::{exit, Command};

const USAGE: &str = "\
Glyph 0.9 - a language AI can write and humans can trust

usage: glyph <command> <file.gly> [options]

commands:
  run       check the program, then run its `main` function
  test      check the program, then run every `test` block
  check     only run the static checks (names, arity, types, effects)
  effects   print what each function is allowed to do
  build     compile to a native executable (needs a C compiler: cc, or $CC)
  fix       ask Claude to fix errors and failing tests; a fix is accepted only if it
            type-checks, adds no effects, keeps every contract and test, and passes

options:
  --json       machine-readable output, one JSON object per line (for AI agents)
  --native     with `run`: compile to native code first, then run it
  -o <path>    with `build`: where to write the executable (default: file name without .gly)
  --emit-c     with `build`: print the generated C instead of compiling it
  --target <t> with `build` / `run`: `native` (default) or `kernel` (bare metal, booted
               in QEMU; needs qemu, clang and lld)
  --write      with `fix`: apply the verified fix to the file (default: only show it)
  --tries <n>  with `fix`: how many proposals to try (default: 3)
  --model <id> with `fix`: Claude model to use (default: claude-opus-5-5)

`fix` needs ANTHROPIC_API_KEY, or a profile from `ant auth login`.
";

struct Opts {
    cmd: String,
    file: String,
    json: bool,
    native: bool,
    emit_c: bool,
    out: Option<String>,
    target: codegen::Target,
    fix: fix::FixOpts,
}

fn parse_args() -> Opts {
    let mut args = std::env::args().skip(1);
    let mut positional = vec![];
    let (mut json, mut native, mut emit_c, mut out) = (false, false, false, None);
    let mut target = codegen::Target::Hosted;
    let mut fix = fix::FixOpts {
        tries: 3,
        write: false,
        model: fix::DEFAULT_MODEL.into(),
        candidate: None,
    };
    while let Some(a) = args.next() {
        match a.as_str() {
            "--json" => json = true,
            "--native" => native = true,
            "--emit-c" => emit_c = true,
            "-o" => out = args.next(),
            "--target" => match args.next().as_deref() {
                Some("native") => target = codegen::Target::Hosted,
                Some("kernel") => target = codegen::Target::Kernel,
                other => {
                    eprintln!(
                        "unknown target `{}`: use `native` or `kernel`",
                        other.unwrap_or("")
                    );
                    exit(2);
                }
            },
            "--write" => fix.write = true,
            "--tries" => match args.next().and_then(|n| n.parse().ok()) {
                Some(n) if n > 0 => fix.tries = n,
                _ => {
                    eprintln!("--tries needs a positive number");
                    exit(2);
                }
            },
            "--model" => fix.model = args.next().unwrap_or_default(),
            "--candidate" => fix.candidate = args.next(),
            "-h" | "--help" => {
                print!("{}", USAGE);
                exit(0);
            }
            _ if a.starts_with('-') => {
                eprint!("unknown option `{}`\n\n{}", a, USAGE);
                exit(2);
            }
            _ => positional.push(a),
        }
    }
    if positional.len() != 2 {
        eprint!("{}", USAGE);
        exit(2);
    }
    let file = positional.pop().unwrap();
    let cmd = positional.pop().unwrap();
    Opts {
        cmd,
        file,
        json,
        native,
        emit_c,
        out,
        target,
        fix,
    }
}

fn main() {
    let opts = parse_args();
    let src = match std::fs::read_to_string(&opts.file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {}", opts.file, e);
            exit(2);
        }
    };
    // Deep recursion in interpreted programs needs a bigger stack than the main thread has.
    let code = std::thread::Builder::new()
        .stack_size(1 << 29)
        .spawn(move || run(&opts, &src))
        .expect("failed to start interpreter thread")
        .join()
        .unwrap_or(1);
    exit(code);
}

fn report(errs: &[Error], sources: &Sources, json: bool) {
    for e in errs {
        if json {
            println!("{}", e.to_json(sources));
        } else {
            eprint!("{}", e.render(sources));
        }
    }
}

fn run(o: &Opts, src: &str) -> i32 {
    let (file, json) = (o.file.as_str(), o.json);
    if !["run", "test", "check", "effects", "build", "fix"].contains(&o.cmd.as_str()) {
        eprint!("unknown command `{}`\n\n{}", o.cmd, USAGE);
        return 2;
    }
    // `fix` runs on broken files too, so it comes before the checks below.
    if o.cmd == "fix" {
        return fix::run(file, src, &o.fix);
    }
    let (prog, sources, mut errs) = match loader::load(file, src) {
        Ok(l) => (l.program, l.sources, l.errors),
        Err((errs, sources)) => {
            report(&errs, &sources, json);
            return 1;
        }
    };
    let checked = check::check(&prog);
    if let Err(more) = &checked {
        errs.extend(more.iter().cloned());
    }
    if !errs.is_empty() {
        report(&errs, &sources, json);
        return 1;
    }
    let types = checked.expect("no errors");

    match o.cmd.as_str() {
        "check" => {
            if json {
                println!(
                    "{{\"ok\":true,\"functions\":{},\"tests\":{}}}",
                    prog.funcs.len(),
                    prog.tests.len()
                );
            } else {
                eprintln!(
                    "ok: {} functions, {} tests, no errors",
                    prog.funcs.len(),
                    prog.tests.len()
                );
            }
            0
        }
        "effects" => {
            for f in &prog.funcs {
                let effs: Vec<&str> = f.effects.iter().map(|e| e.0.as_str()).collect();
                if json {
                    let list: Vec<String> = effs.iter().map(|e| json_str(e)).collect();
                    println!(
                        "{{\"function\":{},\"effects\":[{}]}}",
                        json_str(&f.name),
                        list.join(",")
                    );
                } else if effs.is_empty() {
                    println!("{:<20} pure", f.name);
                } else {
                    println!("{:<20} {}", f.name, effs.join(", "));
                }
            }
            0
        }
        "build" => {
            let c = match codegen::emit_c(&prog, &types, &sources, o.target) {
                Ok(c) => c,
                Err(e) => {
                    report(&[e], &sources, json);
                    return 1;
                }
            };
            if o.emit_c {
                print!("{}", c);
                return 0;
            }
            let kernel = o.target == codegen::Target::Kernel;
            let out = PathBuf::from(o.out.clone().unwrap_or_else(|| {
                Path::new(file)
                    .with_extension(if kernel { "elf" } else { "" })
                    .to_string_lossy()
                    .into_owned()
            }));
            match compile_for(o.target, &c, &out) {
                Ok(()) => {
                    if json {
                        println!(
                            "{{\"ok\":true,\"output\":{}}}",
                            json_str(&out.to_string_lossy())
                        );
                    } else if kernel {
                        eprintln!(
                            "built {} (kernel image; run it with `glyph run {} --target kernel`)",
                            out.display(),
                            file
                        );
                    } else {
                        eprintln!("built {} (native)", out.display());
                    }
                    0
                }
                Err(msg) => {
                    eprintln!("error: {}", msg);
                    1
                }
            }
        }
        "run" if o.native || o.target == codegen::Target::Kernel => {
            let c = match codegen::emit_c(&prog, &types, &sources, o.target) {
                Ok(c) => c,
                Err(e) => {
                    report(&[e], &sources, json);
                    return 1;
                }
            };
            let exe = std::env::temp_dir().join(format!("glyph-run-{}", std::process::id()));
            if let Err(msg) = compile_for(o.target, &c, &exe) {
                eprintln!("error: {}", msg);
                return 1;
            }
            let status = match o.target {
                codegen::Target::Hosted => Command::new(&exe).status(),
                codegen::Target::Kernel => qemu(&exe).status(),
            };
            let _ = std::fs::remove_file(&exe);
            match status {
                Ok(s) => s.code().unwrap_or(1),
                Err(e) => {
                    eprintln!("error: cannot run {}: {}", exe.display(), e);
                    1
                }
            }
        }
        "run" => {
            let Some(main) = prog.funcs.iter().find(|f| f.name == "main") else {
                report(
                    &[
                        Error::new("name", Pos::new(1, 1, 0), "no `main` function to run")
                            .hint("add `fn main() uses io { ... }`, or use `glyph test`"),
                    ],
                    &sources,
                    json,
                );
                return 1;
            };
            if !main.params.is_empty() {
                report(
                    &[Error::new(
                        "type",
                        main.pos,
                        "`main` must not take parameters",
                    )],
                    &sources,
                    json,
                );
                return 1;
            }
            match interp::Interp::new(&prog).call("main", vec![], main.pos) {
                Ok(_) => 0,
                Err(e) => {
                    report(&[e], &sources, json);
                    1
                }
            }
        }
        _ => {
            let mut failed = 0;
            for t in &prog.tests {
                let res = interp::Interp::new(&prog).run_test(t);
                if json {
                    let err = res
                        .as_ref()
                        .err()
                        .map(|e| e.to_json(&sources))
                        .unwrap_or_else(|| "null".into());
                    println!(
                        "{{\"test\":{},\"ok\":{},\"error\":{}}}",
                        json_str(&t.name),
                        res.is_ok(),
                        err
                    );
                } else {
                    match &res {
                        Ok(()) => eprintln!("  PASS  {}", t.name),
                        Err(e) => {
                            eprintln!("  FAIL  {}", t.name);
                            eprint!("{}", e.render(&sources));
                        }
                    }
                }
                if res.is_err() {
                    failed += 1;
                }
            }
            if !json {
                eprintln!("\n{} passed, {} failed", prog.tests.len() - failed, failed);
            }
            if failed > 0 {
                1
            } else {
                0
            }
        }
    }
}

fn compile_for(target: codegen::Target, c: &str, out: &Path) -> Result<(), String> {
    match target {
        codegen::Target::Hosted => compile_c(c, out),
        codegen::Target::Kernel => compile_kernel(c, out),
    }
}

/// Where the kernel image is loaded and how memory is laid out on QEMU's `virt`
/// machine (RAM starts at 0x40000000; we use 256 MB of it).
const KERNEL_LINKER_SCRIPT: &str = "\
ENTRY(_start)
SECTIONS {
  . = 0x40080000;
  .text : { KEEP(*(.text.boot)) *(.text .text.*) }
  .rodata : { *(.rodata .rodata.*) }
  .data : { *(.data .data.*) }
  . = ALIGN(16);
  __bss_start = .;
  .bss (NOLOAD) : { *(.bss .bss.* COMMON) }
  . = ALIGN(16);
  __bss_end = .;
  . += 0x1000000;
  __stack_top = .;
  __heap_start = .;
  __heap_end = 0x50000000;
}
";

/// A clang that can build for bare-metal AArch64 (Apple's can't link it): $GLYPH_KERNEL_CC,
/// then Homebrew's LLVM, then `clang`.
fn kernel_cc() -> String {
    if let Ok(cc) = std::env::var("GLYPH_KERNEL_CC") {
        return cc;
    }
    [
        "/opt/homebrew/opt/llvm/bin/clang",
        "/usr/local/opt/llvm/bin/clang",
    ]
    .iter()
    .find(|p| Path::new(p).exists())
    .map_or_else(|| "clang".to_string(), |p| p.to_string())
}

/// Compiles a kernel image (an ELF file QEMU can boot) with clang and lld.
fn compile_kernel(c: &str, out: &Path) -> Result<(), String> {
    let tmp = std::env::temp_dir();
    let id = std::process::id();
    let c_file = tmp.join(format!("glyph-kernel-{}.c", id));
    let ld_file = tmp.join(format!("glyph-kernel-{}.ld", id));
    std::fs::write(&c_file, c).map_err(|e| format!("cannot write {}: {}", c_file.display(), e))?;
    std::fs::write(&ld_file, KERNEL_LINKER_SCRIPT)
        .map_err(|e| format!("cannot write {}: {}", ld_file.display(), e))?;
    let cc = kernel_cc();
    let result = Command::new(&cc)
        .args([
            "--target=aarch64-none-elf",
            "-ffreestanding",
            "-fno-builtin",
            "-nostdlib",
            "-mstrict-align",
            "-fno-stack-protector",
            "-O2",
            "-w",
            "-fuse-ld=lld",
        ])
        .arg(format!("-Wl,-T,{}", ld_file.display()))
        .arg("-o")
        .arg(out)
        .arg(&c_file)
        .output();
    let _ = std::fs::remove_file(&c_file);
    let _ = std::fs::remove_file(&ld_file);
    match result {
        Err(e) => Err(format!(
            "cannot start `{}`: {} (the kernel target needs clang and lld: `brew install llvm lld`, or set $GLYPH_KERNEL_CC)",
            cc, e
        )),
        Ok(r) if !r.status.success() => Err(format!(
            "building the kernel image failed (it needs clang and lld: `brew install llvm lld`):\n{}",
            String::from_utf8_lossy(&r.stderr)
        )),
        Ok(_) => Ok(()),
    }
}

/// Boots a kernel image in QEMU, with the serial port on this terminal. Semihosting
/// lets the kernel's exit code become QEMU's.
fn qemu(kernel: &Path) -> Command {
    let bin = std::env::var("GLYPH_QEMU").unwrap_or_else(|_| "qemu-system-aarch64".into());
    let mut cmd = Command::new(bin);
    cmd.args([
        "-M",
        "virt",
        "-cpu",
        "cortex-a72",
        "-m",
        "256M",
        "-nographic",
        "-no-reboot",
        "-semihosting-config",
        "enable=on,target=native",
        "-kernel",
    ])
    .arg(kernel);
    cmd
}

/// Writes the generated C to a temporary file and compiles it with the system C compiler.
fn compile_c(c: &str, out: &Path) -> Result<(), String> {
    let c_file = std::env::temp_dir().join(format!("glyph-{}.c", std::process::id()));
    std::fs::write(&c_file, c).map_err(|e| format!("cannot write {}: {}", c_file.display(), e))?;
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    // Extra flags, e.g. GLYPH_CFLAGS="-fsanitize=address -g" to check memory safety.
    let extra = std::env::var("GLYPH_CFLAGS").unwrap_or_default();
    let result = Command::new(&cc)
        .args(["-O2", "-w"])
        .args(extra.split_whitespace())
        .arg("-o")
        .arg(out)
        .arg(&c_file)
        .output();
    let _ = std::fs::remove_file(&c_file);
    match result {
        Err(e) => Err(format!(
            "cannot start the C compiler `{}`: {} (install clang or gcc, or set $CC)",
            cc, e
        )),
        Ok(r) if !r.status.success() => Err(format!(
            "the C compiler failed; this is a bug in Glyph, please report it:\n{}",
            String::from_utf8_lossy(&r.stderr)
        )),
        Ok(_) => Ok(()),
    }
}
