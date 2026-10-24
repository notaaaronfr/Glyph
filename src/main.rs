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
Glyph 0.12 - a language AI can write and humans can trust

usage: glyph <command> <file.gly> [options]
       glyph run <app.gly>... --target kernel     (several apps at once on Glyph OS)

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
    /// All files given; more than one only for Glyph OS (several apps).
    files: Vec<String>,
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
    if positional.len() < 2 {
        eprint!("{}", USAGE);
        exit(2);
    }
    let cmd = positional.remove(0);
    let files = positional;
    let file = files[0].clone();
    if files.len() > 1 && !(target == codegen::Target::Kernel && (cmd == "run" || cmd == "build")) {
        eprintln!("several files can only be given to `run` or `build` with `--target kernel`, as apps for Glyph OS");
        exit(2);
    }
    Opts {
        cmd,
        file,
        files,
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
    if o.target == codegen::Target::Kernel && (o.cmd == "run" || o.cmd == "build") {
        return glyph_os(o);
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
            let out = PathBuf::from(o.out.clone().unwrap_or_else(|| {
                Path::new(file)
                    .with_extension("")
                    .to_string_lossy()
                    .into_owned()
            }));
            match compile_c(&c, &out) {
                Ok(()) => {
                    if json {
                        println!(
                            "{{\"ok\":true,\"output\":{}}}",
                            json_str(&out.to_string_lossy())
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
        "run" if o.native => {
            let c = match codegen::emit_c(&prog, &types, &sources, o.target) {
                Ok(c) => c,
                Err(e) => {
                    report(&[e], &sources, json);
                    return 1;
                }
            };
            let exe = std::env::temp_dir().join(format!("glyph-run-{}", std::process::id()));
            if let Err(msg) = compile_c(&c, &exe) {
                eprintln!("error: {}", msg);
                return 1;
            }
            let status = Command::new(&exe).status();
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

const KERNEL: &str = include_str!("kernel.c");
const KERNEL_API: &str = include_str!("kernel_api.h");
const MAX_APPS: usize = 7;

/// `run` / `build` with `--target kernel`: each file is an app; they are compiled
/// separately and linked with the kernel into one bootable image.
fn glyph_os(o: &Opts) -> i32 {
    if o.files.len() > MAX_APPS {
        eprintln!("error: Glyph OS runs at most {} apps at once", MAX_APPS);
        return 2;
    }
    let mut units = vec![];
    let mut table = vec![];
    let mut decls = String::new();
    for (i, file) in o.files.iter().enumerate() {
        let src = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: cannot read {}: {}", file, e);
                return 2;
            }
        };
        let (prog, sources, mut errs) = match loader::load(file, &src) {
            Ok(l) => (l.program, l.sources, l.errors),
            Err((errs, sources)) => {
                report(&errs, &sources, o.json);
                return 1;
            }
        };
        let checked = check::check(&prog);
        if let Err(more) = &checked {
            errs.extend(more.iter().cloned());
        }
        if !errs.is_empty() {
            report(&errs, &sources, o.json);
            return 1;
        }
        let c = match codegen::emit_c(&prog, &checked.expect("no errors"), &sources, o.target) {
            Ok(c) => c,
            Err(e) => {
                report(&[e], &sources, o.json);
                return 1;
            }
        };
        units.push(format!(
            "#define TN_APP_ENTRY tn_app_main_{}
{}",
            i, c
        ));
        let name = Path::new(file)
            .file_stem()
            .map_or("app".into(), |s| s.to_string_lossy().into_owned());
        // The kernel grants each app what its `main` declares, and nothing more.
        let mut effects: Vec<String> = prog
            .funcs
            .iter()
            .find(|f| f.name == "main")
            .map(|m| m.effects.iter().map(|e| e.0.clone()).collect())
            .unwrap_or_default();
        if let Some(granted) = grant_override(&name) {
            effects = granted;
        }
        let perms: Vec<&str> = effects
            .iter()
            .filter_map(|e| match e.as_str() {
                "io" => Some("TN_PERM_IO"),
                "time" => Some("TN_PERM_TIME"),
                "rand" => Some("TN_PERM_RAND"),
                _ => None, // emit_c already rejected effects the kernel lacks
            })
            .collect();
        let perms = if perms.is_empty() {
            "0".to_string()
        } else {
            perms.join(" | ")
        };
        decls += &format!("void tn_app_main_{}(void);\n", i);
        table.push(format!("{{{:?}, tn_app_main_{}, {}}}", name, i, perms));
    }
    let kernel = format!(
        "{}{}\n{}const tn_app tn_apps[] = {{{}}};\nconst int tn_app_count = {};\n",
        KERNEL_API,
        KERNEL,
        decls,
        table.join(", "),
        table.len()
    );
    units.insert(0, kernel);

    let image = if o.cmd == "build" {
        PathBuf::from(o.out.clone().unwrap_or_else(|| {
            Path::new(&o.file)
                .with_extension("elf")
                .to_string_lossy()
                .into_owned()
        }))
    } else {
        std::env::temp_dir().join(format!("glyph-os-{}.elf", std::process::id()))
    };
    if let Err(msg) = compile_kernel(&units, &image) {
        eprintln!("error: {}", msg);
        return 1;
    }
    if o.cmd == "build" {
        let run = format!("glyph run {} --target kernel", o.files.join(" "));
        if o.json {
            println!(
                "{{\"ok\":true,\"output\":{}}}",
                json_str(&image.to_string_lossy())
            );
        } else {
            eprintln!(
                "built {} (Glyph OS image with {} app(s); run it with `{}`)",
                image.display(),
                o.files.len(),
                run
            );
        }
        return 0;
    }
    let status = qemu(&image).status();
    let _ = std::fs::remove_file(&image);
    match status {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!(
                "error: cannot start QEMU: {} (install it with `brew install qemu`)",
                e
            );
            1
        }
    }
}

/// Where the kernel image is loaded and how memory is laid out on QEMU's `virt`
/// machine (RAM starts at 0x40000000; we use 256 MB of it).
const KERNEL_LINKER_SCRIPT: &str = "\
ENTRY(_start)
SECTIONS {
  . = 0x40080000;
  __text_start = .;
  .text : { KEEP(*(.text.boot)) *(.text .text.*) }
  . = ALIGN(4096);
  __text_end = .;
  .rodata : { *(.rodata .rodata.*) *(.eh_frame*) }
  . = ALIGN(4096);
  __rodata_end = .;
  .data : { *(.data .data.*) *(.got*) }
  . = ALIGN(16);
  __bss_start = .;
  .bss (NOLOAD) : { *(.bss .bss.* COMMON) }
  . = ALIGN(16);
  __bss_end = .;
  . = ALIGN(0x200000);
  __image_end = .;
  __stacks_start = .;
  __stack_top = . + 0xa00000;
  . += 0x5000000;
  __heap_start = .;
  __heap_end = 0x50000000;
}
";

/// For testing the kernel's checks: GLYPH_KERNEL_GRANT="clock=io;busy=" makes the
/// kernel grant those apps only the listed effects, as if their compiled code had been
/// changed to do more than it declares.
fn grant_override(app: &str) -> Option<Vec<String>> {
    let spec = std::env::var("GLYPH_KERNEL_GRANT").ok()?;
    spec.split(';').find_map(|entry| {
        let (name, effects) = entry.split_once('=')?;
        (name.trim() == app).then(|| {
            effects
                .split(',')
                .map(|e| e.trim().to_string())
                .filter(|e| !e.is_empty())
                .collect()
        })
    })
}

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

/// Compiles a kernel image (an ELF file QEMU can boot) from C files with clang and lld.
fn compile_kernel(units: &[String], out: &Path) -> Result<(), String> {
    let tmp = std::env::temp_dir();
    let id = std::process::id();
    let mut c_files = vec![];
    for (i, c) in units.iter().enumerate() {
        let f = tmp.join(format!("glyph-kernel-{}-{}.c", id, i));
        std::fs::write(&f, c).map_err(|e| format!("cannot write {}: {}", f.display(), e))?;
        c_files.push(f);
    }
    let ld_file = tmp.join(format!("glyph-kernel-{}.ld", id));
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
        .args(
            std::env::var("GLYPH_KERNEL_CFLAGS")
                .unwrap_or_default()
                .split_whitespace(),
        )
        .arg("-o")
        .arg(out)
        .args(&c_files)
        .output();
    for f in &c_files {
        let _ = std::fs::remove_file(f);
    }
    let _ = std::fs::remove_file(&ld_file);
    match result {
        Err(e) => Err(format!(
            "cannot start `{}`: {} (the kernel target needs clang and lld: `brew install llvm lld`, or set $GLYPH_KERNEL_CC)",
            cc, e
        )),
        Ok(r) if !r.status.success() => Err(format!(
            "building the kernel image failed (if clang or lld is missing: `brew install llvm lld`):\n{}",
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
        "virt,gic-version=2",
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
