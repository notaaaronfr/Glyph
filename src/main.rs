mod fix;

use glyph_front::{check, json_str, load, Error, Pos, Sources};
use glyph_native::{emit_c, Target};
use glyph_run::Interp;
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
    target: Target,
    fix: fix::FixOpts,
}

fn parse_args() -> Opts {
    let mut args = std::env::args().skip(1);
    let mut positional = vec![];
    let (mut json, mut native, mut emit_c, mut out) = (false, false, false, None);
    let mut target = Target::Hosted;
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
                Some("native") => target = Target::Hosted,
                Some("kernel") => target = Target::Kernel,
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
    if files.len() > 1 && !(target == Target::Kernel && (cmd == "run" || cmd == "build")) {
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
    if o.target == Target::Kernel && (o.cmd == "run" || o.cmd == "build") {
        return glyph_os(o);
    }
    let (prog, sources, mut errs) = match load(file, src) {
        Ok(l) => (l.program, l.sources, l.errors),
        Err((errs, sources)) => {
            report(&errs, &sources, json);
            return 1;
        }
    };
    let checked = check(&prog);
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
            let c = match emit_c(&prog, &types, &sources, o.target) {
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
            let c = match emit_c(&prog, &types, &sources, o.target) {
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
            match Interp::new(&prog).call("main", vec![], main.pos) {
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
                let res = Interp::new(&prog).run_test(t);
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

/// `run` / `build` with `--target kernel`: each file is an app; they are compiled
/// separately and linked with the kernel into one bootable image.
fn glyph_os(o: &Opts) -> i32 {
    if o.files.len() > glyph_kernel::MAX_APPS {
        eprintln!(
            "error: Glyph OS runs at most {} apps at once",
            glyph_kernel::MAX_APPS
        );
        return 2;
    }
    let mut apps = vec![];
    for file in &o.files {
        let src = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: cannot read {}: {}", file, e);
                return 2;
            }
        };
        let (prog, sources, mut errs) = match load(file, &src) {
            Ok(l) => (l.program, l.sources, l.errors),
            Err((errs, sources)) => {
                report(&errs, &sources, o.json);
                return 1;
            }
        };
        let checked = check(&prog);
        if let Err(more) = &checked {
            errs.extend(more.iter().cloned());
        }
        if !errs.is_empty() {
            report(&errs, &sources, o.json);
            return 1;
        }
        let c = match emit_c(&prog, &checked.expect("no errors"), &sources, o.target) {
            Ok(c) => c,
            Err(e) => {
                report(&[e], &sources, o.json);
                return 1;
            }
        };
        let name = Path::new(file)
            .file_stem()
            .map_or("app".into(), |s| s.to_string_lossy().into_owned());
        apps.push(glyph_kernel::App::new(name, c, &prog));
    }
    let units = glyph_kernel::units(&apps);

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
    if let Err(msg) = glyph_kernel::build_image(&units, &image) {
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
    let status = glyph_kernel::qemu(&image).status();
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
