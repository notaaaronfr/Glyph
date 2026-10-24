//! Glyph OS: the bare-metal kernel, the interface apps are compiled against, and
//! the code that turns checked programs into a bootable image.
//!
//! The kernel enforces each app's declared effects as permissions, and the grant
//! table it enforces is built here, so this crate is part of the trusted base.

#![forbid(unsafe_code)]

use glyph_front::ast::Program;
use std::path::Path;
use std::process::Command;

/// What the kernel provides to apps; each app is compiled against it.
pub const API: &str = include_str!("kernel_api.h");
const KERNEL: &str = include_str!("kernel.c");
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

/// The C units of a bootable image: the kernel with its grant table first, then
/// each app, compiled separately.
pub fn units(apps: &[App]) -> Vec<String> {
    let mut units = vec![];
    let mut table = vec![];
    let mut decls = String::new();
    for (i, app) in apps.iter().enumerate() {
        units.push(format!("#define TN_APP_ENTRY tn_app_main_{i}\n{}", app.c));
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
        decls += &format!("void tn_app_main_{i}(void);\n");
        table.push(format!("{{{:?}, tn_app_main_{i}, {perms}}}", app.name));
    }
    let kernel = format!(
        "{API}{KERNEL}\n{decls}const tn_app tn_apps[] = {{{}}};\nconst int tn_app_count = {};\n",
        table.join(", "),
        table.len()
    );
    units.insert(0, kernel);
    units
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

/// Compiles a kernel image (an ELF file QEMU can boot) from C files with clang and lld.
pub fn build_image(units: &[String], out: &Path) -> Result<(), String> {
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
