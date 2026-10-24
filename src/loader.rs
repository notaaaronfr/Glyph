//! Loads a program and the modules it imports into one `Program`.
//!
//! `import shapes.circle` loads `shapes/circle.gly` (relative to the directory of the
//! file being run) and makes its `pub` items available as `circle.name`. Each module
//! is loaded once; import cycles are an error. Items of an imported module get the
//! internal name `shapes.circle.name`, so the rest of the compiler sees one program
//! with unique names. The file that was run keeps plain names.

use crate::ast::*;
use crate::error::{Error, Pos, Sources};
use crate::parser::{parse_file, scan_imports, ModuleNames};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub struct Loaded {
    pub program: Program,
    pub sources: Sources,
    /// Problems that don't stop checking (like using a private item), reported along
    /// with the type checker's errors.
    pub errors: Vec<Error>,
}

/// Loads the program whose main file is `path`, with contents `src`.
pub fn load(path: &str, src: &str) -> Result<Loaded, (Vec<Error>, Sources)> {
    let root_dir = Path::new(path)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let mut l = Loader {
        root_dir,
        sources: Sources::default(),
        modules: HashMap::new(),
        loading: vec![],
        program: Program {
            types: vec![],
            enums: vec![],
            funcs: vec![],
            tests: vec![],
        },
        errors: vec![],
        soft_errors: vec![],
    };
    l.module(None, path.to_string(), src.to_string());
    if l.errors.is_empty() {
        Ok(Loaded {
            program: l.program,
            sources: l.sources,
            errors: l.soft_errors,
        })
    } else {
        Err((l.errors, l.sources))
    }
}

/// What other files may use from a loaded module.
#[derive(Clone)]
struct ModuleInfo {
    names: ModuleNames,
    public: HashSet<String>,
}

struct Loader {
    root_dir: PathBuf,
    sources: Sources,
    modules: HashMap<String, ModuleInfo>,
    /// Modules being loaded, outermost first, to report import cycles.
    loading: Vec<String>,
    program: Program,
    /// Errors that stop loading (syntax, missing modules, cycles).
    errors: Vec<Error>,
    soft_errors: Vec<Error>,
}

impl Loader {
    /// Loads one file. `name` is its module path (`shapes.circle`), or None for the
    /// main file.
    fn module(&mut self, name: Option<&str>, display: String, src: String) -> Option<ModuleInfo> {
        let file = self.sources.0.len();
        self.sources.0.push((display, src.clone()));
        let imports = match scan_imports(&src, file) {
            Ok(i) => i,
            Err(e) => {
                self.errors.push(e);
                return None;
            }
        };

        // Load what this file imports first, so we know the names it may use.
        let mut aliases: HashMap<String, (String, ModuleInfo)> = HashMap::new();
        for (path, pos) in &imports {
            let Some(info) = self.import(path, *pos) else {
                continue;
            };
            let alias = path.rsplit('.').next().unwrap_or(path).to_string();
            if let Some((other, _)) = aliases.get(&alias) {
                if other != path {
                    self.errors.push(Error::new(
                        "name",
                        *pos,
                        format!(
                            "`{}` and `{}` would both be called `{}`",
                            other, path, alias
                        ),
                    ));
                }
                continue;
            }
            aliases.insert(alias, (path.clone(), info));
        }
        if !self.errors.is_empty() {
            return None;
        }

        let names: HashMap<String, ModuleNames> = aliases
            .iter()
            .map(|(a, (_, info))| (a.clone(), info.names.clone()))
            .collect();
        let mut prog = match parse_file(&src, file, &names, name.is_none()) {
            Ok(p) => p,
            Err(e) => {
                self.errors.push(e);
                return None;
            }
        };
        let info = ModuleInfo {
            names: ModuleNames::of(&prog),
            public: public_names(&prog),
        };

        let mut r = Renamer {
            prefix: name.map(|n| format!("{}.", n)).unwrap_or_default(),
            own: {
                let mut own = HashSet::new();
                let n = &info.names;
                for set in [&n.funcs, &n.types, &n.enums, &n.variants] {
                    own.extend(set.iter().cloned());
                }
                own
            },
            aliases,
            errors: vec![],
        };
        r.program(&mut prog, name);
        self.soft_errors.append(&mut r.errors);

        self.program.types.append(&mut prog.types);
        self.program.enums.append(&mut prog.enums);
        self.program.funcs.append(&mut prog.funcs);
        self.program.tests.append(&mut prog.tests);
        Some(info)
    }

    fn import(&mut self, path: &str, pos: Pos) -> Option<ModuleInfo> {
        if let Some(info) = self.modules.get(path) {
            return Some(info.clone());
        }
        if let Some(i) = self.loading.iter().position(|m| m == path) {
            let mut cycle = self.loading[i..].to_vec();
            cycle.push(path.to_string());
            self.errors.push(
                Error::new("name", pos, format!("import cycle: {}", cycle.join(" -> ")))
                    .hint("move what both modules need into a third module that neither imports"),
            );
            return None;
        }
        let rel: PathBuf = path.split('.').collect::<PathBuf>().with_extension("gly");
        let file = self.root_dir.join(&rel);
        let src = match std::fs::read_to_string(&file) {
            Ok(s) => s,
            Err(_) => {
                self.errors.push(Error::new(
                    "name",
                    pos,
                    format!(
                        "cannot find module `{}` (looked for {})",
                        path,
                        file.display()
                    ),
                ));
                return None;
            }
        };
        self.loading.push(path.to_string());
        let info = self.module(Some(path), file.display().to_string(), src);
        self.loading.pop();
        if let Some(info) = &info {
            self.modules.insert(path.to_string(), info.clone());
        }
        info
    }
}

fn public_names(p: &Program) -> HashSet<String> {
    let mut out = HashSet::new();
    out.extend(p.funcs.iter().filter(|f| f.public).map(|f| f.name.clone()));
    out.extend(p.types.iter().filter(|t| t.public).map(|t| t.name.clone()));
    for e in p.enums.iter().filter(|e| e.public) {
        out.insert(e.name.clone());
        out.extend(e.variants.iter().map(|v| v.name.clone()));
    }
    out
}

/// Rewrites one file's names to the program-wide internal names.
struct Renamer {
    /// `shapes.circle.` for an imported module, empty for the main file.
    prefix: String,
    /// Names declared in this file.
    own: HashSet<String>,
    /// Imported modules: name used in code -> (module path, what it exposes).
    aliases: HashMap<String, (String, ModuleInfo)>,
    errors: Vec<Error>,
}

impl Renamer {
    fn name(&mut self, n: &str, pos: Pos) -> String {
        if let Some((alias, item)) = n.split_once('.') {
            let Some((path, info)) = self.aliases.get(alias) else {
                return n.to_string();
            };
            if !info.public.contains(item) {
                let file = path.replace('.', "/") + ".gly";
                self.errors.push(
                    Error::new(
                        "name",
                        pos,
                        format!("`{}` is private to module `{}`", n, path),
                    )
                    .hint(format!("mark it `pub` in {} to use it here", file)),
                );
            }
            return format!("{}.{}", path, item);
        }
        if self.own.contains(n) {
            format!("{}{}", self.prefix, n)
        } else {
            n.to_string()
        }
    }

    fn ty(&mut self, t: &mut Type, pos: Pos) {
        match t {
            Type::List(x) => self.ty(x, pos),
            Type::Record(n, args) | Type::Enum(n, args) => {
                *n = self.name(n, pos);
                for a in args {
                    self.ty(a, pos);
                }
            }
            _ => {}
        }
    }

    fn program(&mut self, p: &mut Program, module: Option<&str>) {
        for t in &mut p.types {
            t.name = self.name(&t.name, t.pos);
            for (_, ft, pos) in &mut t.fields {
                self.ty(ft, *pos);
            }
        }
        for e in &mut p.enums {
            e.name = self.name(&e.name, e.pos);
            for v in &mut e.variants {
                v.name = self.name(&v.name, v.pos);
                for (_, ft, pos) in &mut v.fields {
                    self.ty(ft, *pos);
                }
            }
        }
        for f in &mut p.funcs {
            f.name = self.name(&f.name, f.pos);
            for param in &mut f.params {
                self.ty(&mut param.ty, f.pos);
            }
            self.ty(&mut f.ret, f.pos);
            for e in f.requires.iter_mut().chain(&mut f.ensures) {
                self.expr(e);
            }
            self.stmts(&mut f.body);
        }
        for t in &mut p.tests {
            if let Some(m) = module {
                t.name = format!("{}: {}", m, t.name);
            }
            self.stmts(&mut t.body);
        }
    }

    fn stmts(&mut self, stmts: &mut [Stmt]) {
        for s in stmts {
            match s {
                Stmt::Let(_, ann, e) => {
                    if let Some(t) = ann {
                        self.ty(t, e.pos);
                    }
                    self.expr(e);
                }
                Stmt::Assign(_, e, _)
                | Stmt::SetField(_, _, e, _)
                | Stmt::Expr(e)
                | Stmt::Assert(e) => self.expr(e),
                Stmt::If(c, t, els) => {
                    self.expr(c);
                    self.stmts(t);
                    if let Some(b) = els {
                        self.stmts(b);
                    }
                }
                Stmt::While(c, b) => {
                    self.expr(c);
                    self.stmts(b);
                }
                Stmt::For(_, e, b, _) => {
                    self.expr(e);
                    self.stmts(b);
                }
                Stmt::Return(e, _) => {
                    if let Some(e) = e {
                        self.expr(e);
                    }
                }
                Stmt::Match(e, arms, _) => {
                    self.expr(e);
                    for arm in arms {
                        if let Pattern::Variant(n, _, pos) = &mut arm.pattern {
                            *n = self.name(n, *pos);
                        }
                        self.stmts(&mut arm.body);
                    }
                }
            }
        }
    }

    fn expr(&mut self, e: &mut Expr) {
        let pos = e.pos;
        match &mut e.kind {
            ExprKind::Call(n, args) => {
                *n = self.name(n, pos);
                args.iter_mut().for_each(|a| self.expr(a));
            }
            ExprKind::Record(n, fields) | ExprKind::Variant(n, fields) => {
                *n = self.name(n, pos);
                fields.iter_mut().for_each(|(_, x, _)| self.expr(x));
            }
            ExprKind::List(xs) => xs.iter_mut().for_each(|x| self.expr(x)),
            ExprKind::Unary(_, x) | ExprKind::Field(x, _) => self.expr(x),
            ExprKind::Binary(_, l, r) | ExprKind::Index(l, r) => {
                self.expr(l);
                self.expr(r);
            }
            ExprKind::Int(_) | ExprKind::Str(_) | ExprKind::Bool(_) | ExprKind::Var(_) => {}
        }
    }
}
