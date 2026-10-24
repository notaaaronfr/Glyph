//! Glyph OS: the bare-metal kernel, the interface apps are compiled against, and
//! the code that turns checked programs into a bootable image.
//!
//! The kernel enforces each app's declared effects as permissions, and the grant
//! table it enforces is built here, so this crate is part of the trusted base.

#![forbid(unsafe_code)]

use glyph_front::ast::Program;
use std::path::Path;
use std::process::Command;

/// What an app sees of the kernel (the ABI, system calls, its own allocator); each
/// app is compiled against it.
pub const API: &str = concat!(include_str!("abi.h"), include_str!("kernel_api.h"));
const ABI: &str = include_str!("abi.h");
const KERNEL: &str = include_str!("kernel.c");
const LIBC: &str = include_str!("libc.c");
/// How many apps one image can run at once.
pub const MAX_APPS: usize = 7;

/// An app compiled for Glyph OS, with the effects the kernel will grant it.
pub struct App {
    name: String,
    c: String,
    grant: Vec<String>,
}

impl App {
    /// Grants the app the effects its `main` declares, and nothing more.
    pub fn new(name: String, c: String, prog: &Program) -> Self {
        let mut grant: Vec<String> = prog
            .funcs
            .iter()
            .find(|f| f.name == "main")
            .map(|m| m.effects.iter().map(|e| e.0.clone()).collect())
            .unwrap_or_default();
        if let Some(only) = grant_override(&name) {
            grant.retain(|e| only.contains(e));
        }
        App { name, c, grant }
    }
}

/// The C units of a bootable image: the kernel with its grant table first, the
/// shared C library, then each app, compiled separately.
pub fn units(apps: &[App]) -> Vec<String> {
    let mut units = vec![];
    let mut table = vec![];
    let mut decls = String::new();
    for (i, app) in apps.iter().enumerate() {
        units.push(format!(
            "#define TN_APP_ENTRY tn_app_main_{i}\n#define TN_APP_INDEX {i}\n{}",
            app.c
        ));
        let perms: Vec<&str> = app
            .grant
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
        decls += &format!(
            "void tn_app_main_{i}(void);\nextern char __app{i}_start[], __app{i}_text_end[], __app{i}_ro_end[], __app{i}_end[];\n"
        );
        table.push(format!(
            "{{{:?}, tn_app_main_{i}, {perms}, __app{i}_start, __app{i}_text_end, __app{i}_ro_end, __app{i}_end}}",
            app.name
        ));
    }
    let kernel = format!(
        "{ABI}{KERNEL}\n{decls}const tn_app tn_apps[] = {{{}}};\nconst int tn_app_count = {};\n",
        table.join(", "),
        table.len()
    );
    units.insert(0, kernel);
    units.insert(1, format!("{ABI}{LIBC}"));
    units
}

/// Where the kernel image is loaded and how memory is laid out on QEMU's `virt`
/// machine (RAM starts at 0x40000000; we use 256 MB of it). The kernel comes first,
/// then the shared C library, then each app in its own page-aligned region, so the
/// kernel can open exactly an app's own pages to it. `apps` is the number of apps.
fn linker_script(apps: usize) -> String {
    let mut script = String::from(
        "ENTRY(_start)
SECTIONS {
  . = 0x40080000;
  __text_start = .;
  .text : { KEEP(*(.text.boot)) *kernel.o(.text .text.*) }
  . = ALIGN(4096);
  __text_end = .;
  .rodata : { *kernel.o(.rodata .rodata.*) }
  . = ALIGN(4096);
  __rodata_end = .;
  .data : { *kernel.o(.data .data.*) *(.got .got.*) }
  . = ALIGN(16);
  __bss_start = .;
  .bss (NOLOAD) : { *kernel.o(.bss .bss.* COMMON) }
  . = ALIGN(16);
  __bss_end = .;
  . = ALIGN(4096);
  __shared_start = .;
  .shared.text : { *libc.o(.text .text.*) }
  . = ALIGN(4096);
  __shared_text_end = .;
  .shared.rodata : { *libc.o(.rodata .rodata.*) }
  . = ALIGN(4096);
  __shared_end = .;
",
    );
    for i in 0..apps {
        script += &format!(
            "  __app{i}_start = .;
  .app{i}.text : {{ *app{i}.o(.text .text.*) }}
  . = ALIGN(4096);
  __app{i}_text_end = .;
  .app{i}.rodata : {{ *app{i}.o(.rodata .rodata.*) }}
  . = ALIGN(4096);
  __app{i}_ro_end = .;
  .app{i}.data : {{ *app{i}.o(.data .data.* .bss .bss.* COMMON) }}
  . = ALIGN(4096);
  __app{i}_end = .;
"
        );
    }
    script += "  . = ALIGN(0x200000);
  __image_end = .;
  __stacks_start = .;
  __stack_top = . + 0xc00000;
  . += 0x6000000;
  __heap_start = .;
  __heap_end = 0x50000000;
}
";
    script
}

/// For testing the kernel's checks: GLYPH_KERNEL_GRANT="clock=io;busy=" makes the
/// kernel grant those apps only the listed effects, as if their compiled code had been
/// changed to do more than it declares. It can only take effects away: an effect the
/// app's `main` doesn't declare is never granted, whatever the variable says.
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

/// Compiles a kernel image (an ELF file QEMU can boot) from the C units of `units`
/// with clang and lld: each unit on its own, then linked by the generated script.
pub fn build_image(units: &[String], out: &Path) -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("glyph-kernel-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let result = build_in(&dir, units, out);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn build_in(dir: &Path, units: &[String], out: &Path) -> Result<(), String> {
    let cc = kernel_cc();
    let extra = std::env::var("GLYPH_KERNEL_CFLAGS").unwrap_or_default();
    let run = |cmd: &mut Command| {
        match cmd.output() {
        Err(e) => Err(format!(
            "cannot start `{cc}`: {e} (the kernel target needs clang and lld: `brew install llvm lld`, or set $GLYPH_KERNEL_CC)"
        )),
        Ok(r) if !r.status.success() => Err(format!(
            "building the kernel image failed (if clang or lld is missing: `brew install llvm lld`):\n{}",
            String::from_utf8_lossy(&r.stderr)
        )),
        Ok(_) => Ok(()),
    }
    };
    let mut objects = vec![];
    for (i, c) in units.iter().enumerate() {
        let name = match i {
            0 => "kernel".to_string(),
            1 => "libc".to_string(),
            _ => format!("app{}", i - 2),
        };
        let src = dir.join(format!("{name}.c"));
        let obj = dir.join(format!("{name}.o"));
        std::fs::write(&src, c).map_err(|e| format!("cannot write {}: {e}", src.display()))?;
        run(Command::new(&cc)
            .args([
                "--target=aarch64-none-elf",
                "-ffreestanding",
                "-fno-builtin",
                "-mstrict-align",
                "-fno-stack-protector",
                "-fno-asynchronous-unwind-tables",
                "-fno-unwind-tables",
                "-O2",
                "-w",
                "-c",
            ])
            .args(extra.split_whitespace())
            .arg("-o")
            .arg(&obj)
            .arg(&src))?;
        objects.push(obj);
    }
    let script = dir.join("kernel.ld");
    std::fs::write(&script, linker_script(units.len() - 2))
        .map_err(|e| format!("cannot write {}: {e}", script.display()))?;
    run(Command::new(&cc)
        .args(["--target=aarch64-none-elf", "-nostdlib", "-fuse-ld=lld"])
        .arg(format!("-Wl,-T,{}", script.display()))
        .arg("-o")
        .arg(out)
        .args(&objects))
}

/// Boots a kernel image in QEMU, with the serial port on this terminal. Semihosting
/// lets the kernel's exit code become QEMU's.
pub fn qemu(image: &Path) -> Command {
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
    .arg(image);
    cmd
}
