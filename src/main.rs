mod ast;
mod check;
mod codegen;
mod error;
mod fix;
mod interp;
mod lexer;
mod parser;

use error::{json_str, Error, Pos};
use std::path::{Path, PathBuf};
use std::process::{exit, Command};

const USAGE: &str = "\
Glyph 0.5 - a language AI can write and humans can trust

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
    fix: fix::FixOpts,
}

fn parse_args() -> Opts {
    let mut args = std::env::args().skip(1);
    let mut positional = vec![];
    let (mut json, mut native, mut emit_c, mut out) = (false, false, false, None);
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

fn report(errs: &[Error], file: &str, src: &str, json: bool) {
    for e in errs {
        if json {
            println!("{}", e.to_json(file));
        } else {
            eprint!("{}", e.render(file, src));
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
    let prog = match parser::parse(src) {
        Ok(p) => p,
        Err(e) => {
            report(&[e], file, src, json);
            return 1;
        }
    };
    let types = match check::check(&prog) {
        Ok(t) => t,
        Err(errs) => {
            report(&errs, file, src, json);
            return 1;
        }
    };

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
            let c = match codegen::emit_c(&prog, &types, file, src) {
                Ok(c) => c,
                Err(e) => {
                    report(&[e], file, src, json);
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
            let c = match codegen::emit_c(&prog, &types, file, src) {
                Ok(c) => c,
                Err(e) => {
                    report(&[e], file, src, json);
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
                        Error::new("name", Pos { line: 1, col: 1 }, "no `main` function to run")
                            .hint("add `fn main() uses io { ... }`, or use `glyph test`"),
                    ],
                    file,
                    src,
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
                    file,
                    src,
                    json,
                );
                return 1;
            }
            match interp::Interp::new(&prog).call("main", vec![], main.pos) {
                Ok(_) => 0,
                Err(e) => {
                    report(&[e], file, src, json);
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
                        .map(|e| e.to_json(file))
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
                            eprint!("{}", e.render(file, src));
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

/// Writes the generated C to a temporary file and compiles it with the system C compiler.
fn compile_c(c: &str, out: &Path) -> Result<(), String> {
    let c_file = std::env::temp_dir().join(format!("glyph-{}.c", std::process::id()));
    std::fs::write(&c_file, c).map_err(|e| format!("cannot write {}: {}", c_file.display(), e))?;
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    let result = Command::new(&cc)
        .args(["-O2", "-w", "-o"])
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
