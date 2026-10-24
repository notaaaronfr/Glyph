use crate::ast::*;
use crate::error::{Error, Pos};
use crate::lexer::{lex, Tok, Token};
use std::collections::{HashMap, HashSet};

const KEYWORDS: [&str; 23] = [
    "fn", "let", "if", "else", "while", "for", "in", "return", "true", "false", "uses", "requires",
    "ensures", "test", "assert", "and", "or", "not", "type", "enum", "match", "import", "pub",
];

/// The names a module declares, so files importing it can parse `module.Name`.
#[derive(Debug, Clone, Default)]
pub struct ModuleNames {
    pub funcs: HashSet<String>,
    pub types: HashSet<String>,
    pub enums: HashSet<String>,
    pub variants: HashSet<String>,
    pub variants_with_fields: HashSet<String>,
}

impl ModuleNames {
    pub fn of(p: &Program) -> Self {
        let mut m = ModuleNames {
            funcs: p.funcs.iter().map(|f| f.name.clone()).collect(),
            types: p.types.iter().map(|t| t.name.clone()).collect(),
            enums: p.enums.iter().map(|e| e.name.clone()).collect(),
            ..Default::default()
        };
        for v in p.enums.iter().flat_map(|e| &e.variants) {
            m.variants.insert(v.name.clone());
            if !v.fields.is_empty() {
                m.variants_with_fields.insert(v.name.clone());
            }
        }
        m
    }

    pub fn contains(&self, n: &str) -> bool {
        self.funcs.contains(n)
            || self.types.contains(n)
            || self.enums.contains(n)
            || self.variants.contains(n)
    }
}

/// The modules a file imports (`import a.b`), found without parsing the whole file.
pub fn scan_imports(src: &str, file: usize) -> Result<Vec<(String, Pos)>, Error> {
    let toks = lex(src, file)?;
    let mut out = vec![];
    let mut i = 0;
    while i < toks.len() {
        if toks[i].tok == Tok::Ident("import".into()) {
            let pos = toks[i].pos;
            let mut path = vec![];
            i += 1;
            while let Some(Tok::Ident(n)) = toks.get(i).map(|t| &t.tok) {
                path.push(n.clone());
                if toks.get(i + 1).map(|t| &t.tok) != Some(&Tok::Sym(".")) {
                    break;
                }
                i += 2;
            }
            if !path.is_empty() {
                out.push((path.join("."), pos));
            }
        }
        i += 1;
    }
    Ok(out)
}

const BUILTIN_TYPES: [&str; 6] = ["Int", "Str", "Bool", "Unit", "Any", "List"];

/// Declarations available in every program.
const PRELUDE: &str = "
enum Option[T] {
  Some { value: T }
  None
}

enum Result[T, E] {
  Ok { value: T }
  Err { error: E }
}
";

/// Parses one file. `aliases` maps each imported module's name (the last part of its
/// path) to what it declares. The prelude's declarations are added if `prelude`.
/// `import` lines are skipped here; the loader reads them with `scan_imports`.
pub fn parse_file(
    src: &str,
    file: usize,
    aliases: &HashMap<String, ModuleNames>,
    prelude: bool,
) -> Result<Program, Error> {
    // Prelude positions are line 0, which marks them as built in.
    let mut prelude_toks = lex(PRELUDE, 0).expect("the prelude lexes");
    for t in &mut prelude_toks {
        t.pos = Pos::default();
    }
    let user_toks = lex(src, file)?;
    let all: Vec<Token> = prelude_toks.iter().chain(&user_toks).cloned().collect();
    // Record types may be used before they are declared, and `Name {` is only a record
    // literal when `Name` is a declared type, so collect the type names up front.
    let names_after = |kw: &str| -> HashSet<String> {
        all.windows(2)
            .filter(|w| w[0].tok == Tok::Ident(kw.into()))
            .filter_map(|w| match &w[1].tok {
                Tok::Ident(n) => Some(n.clone()),
                _ => None,
            })
            .collect()
    };
    let type_names = names_after("type");
    let enum_names = names_after("enum");
    let (variant_names, variants_with_fields) = scan_variants(&all);
    let mut prog = Program {
        types: vec![],
        enums: vec![],
        funcs: vec![],
        tests: vec![],
    };
    let prelude_decls = prelude;
    let mut prelude = Parser {
        toks: prelude_toks,
        i: 0,
        src: PRELUDE,
        type_names: type_names.clone(),
        enum_names: enum_names.clone(),
        variant_names: variant_names.clone(),
        variants_with_fields: variants_with_fields.clone(),
        tparams: vec![],
        aliases: HashMap::new(),
    };
    while prelude.is_kw("enum") {
        let mut e = prelude.enum_decl().expect("the prelude parses");
        e.public = true;
        if prelude_decls {
            prog.enums.push(e);
        }
    }
    let mut p = Parser {
        toks: user_toks,
        i: 0,
        src,
        type_names,
        enum_names,
        variant_names,
        variants_with_fields,
        tparams: vec![],
        aliases: aliases.clone(),
    };
    loop {
        if matches!(p.peek(), Tok::Eof) {
            break;
        }
        if p.eat_kw("import") {
            p.ident()?;
            while p.eat_sym(".") {
                p.ident()?;
            }
            continue;
        }
        let public = p.eat_kw("pub");
        if p.is_kw("type") {
            let mut t = p.type_decl()?;
            t.public = public;
            prog.types.push(t);
        } else if p.is_kw("enum") {
            let mut e = p.enum_decl()?;
            e.public = public;
            prog.enums.push(e);
        } else if p.is_kw("fn") {
            let mut f = p.func()?;
            f.public = public;
            prog.funcs.push(f);
        } else if public {
            return Err(p.err("`pub` goes before `fn`, `type` or `enum`"));
        } else if p.is_kw("test") {
            prog.tests.push(p.test()?);
        } else {
            return Err(p.err(format!(
                "expected `import`, `type`, `enum`, `fn` or `test` at top level, found {}",
                p.describe()
            )));
        }
    }
    Ok(prog)
}

/// Variant names are the identifiers directly inside an `enum { ... }` body (field
/// names and types sit one level deeper). Also records which variants have fields:
/// only those can be followed by `{`, so `if s == Empty {` still parses as an `if`.
fn scan_variants(toks: &[Token]) -> (HashSet<String>, HashSet<String>) {
    let (mut names, mut with_fields) = (HashSet::new(), HashSet::new());
    let mut i = 0;
    while i + 2 < toks.len() {
        if toks[i].tok != Tok::Ident("enum".into()) {
            i += 1;
            continue;
        }
        // Skip the name and any type parameters, up to the body's `{`.
        while i < toks.len() && toks[i].tok != Tok::Sym("{") {
            i += 1;
        }
        i += 1;
        let mut depth = 1;
        while i < toks.len() && depth > 0 {
            match &toks[i].tok {
                Tok::Sym("{") => depth += 1,
                Tok::Sym("}") => depth -= 1,
                Tok::Ident(n) if depth == 1 => {
                    names.insert(n.clone());
                    if toks.get(i + 1).is_some_and(|t| t.tok == Tok::Sym("{")) {
                        with_fields.insert(n.clone());
                    }
                }
                Tok::Eof => break,
                _ => {}
            }
            i += 1;
        }
    }
    (names, with_fields)
}

struct Parser<'a> {
    toks: Vec<Token>,
    i: usize,
    src: &'a str,
    type_names: HashSet<String>,
    enum_names: HashSet<String>,
    variant_names: HashSet<String>,
    variants_with_fields: HashSet<String>,
    /// Type parameters in scope while parsing a generic declaration or function.
    tparams: Vec<String>,
    /// Imported modules, by the name they are used with.
    aliases: HashMap<String, ModuleNames>,
}

fn bx(e: Expr) -> Box<Expr> {
    Box::new(e)
}

impl<'a> Parser<'a> {
    fn peek(&self) -> &Tok {
        &self.toks[self.i].tok
    }

    fn tok(&self) -> &Token {
        &self.toks[self.i]
    }

    fn pos(&self) -> Pos {
        self.toks[self.i].pos
    }

    fn bump(&mut self) -> Token {
        let t = self.toks[self.i].clone();
        if self.i < self.toks.len() - 1 {
            self.i += 1;
        }
        t
    }

    fn is_sym(&self, s: &str) -> bool {
        matches!(self.peek(), Tok::Sym(x) if *x == s)
    }

    fn is_kw(&self, k: &str) -> bool {
        matches!(self.peek(), Tok::Ident(x) if x == k)
    }

    fn eat_sym(&mut self, s: &str) -> bool {
        let hit = self.is_sym(s);
        if hit {
            self.bump();
        }
        hit
    }

    fn eat_kw(&mut self, k: &str) -> bool {
        let hit = self.is_kw(k);
        if hit {
            self.bump();
        }
        hit
    }

    fn expect_sym(&mut self, s: &str) -> Result<(), Error> {
        if self.eat_sym(s) {
            Ok(())
        } else {
            Err(self.err(format!("expected `{}`, found {}", s, self.describe())))
        }
    }

    fn describe(&self) -> String {
        match self.peek() {
            Tok::Ident(x) => format!("`{}`", x),
            Tok::Int(n) => format!("`{}`", n),
            Tok::Str(_) => "a string".into(),
            Tok::Sym(s) => format!("`{}`", s),
            Tok::Eof => "end of file".into(),
        }
    }

    fn err(&self, msg: impl Into<String>) -> Error {
        Error::new("syntax", self.pos(), msg)
    }

    fn ident(&mut self) -> Result<(String, Pos), Error> {
        match self.peek().clone() {
            Tok::Ident(x) if !KEYWORDS.contains(&x.as_str()) => {
                let pos = self.pos();
                self.bump();
                Ok((x, pos))
            }
            _ => Err(self.err(format!("expected a name, found {}", self.describe()))),
        }
    }

    fn func(&mut self) -> Result<Func, Error> {
        let pos = self.pos();
        self.bump();
        let (name, _) = self.ident()?;
        let tparams = self.type_params()?;
        self.tparams = tparams.clone();
        self.expect_sym("(")?;
        let mut params = vec![];
        while !self.is_sym(")") {
            let (pname, ppos) = self.ident()?;
            if !self.eat_sym(":") {
                return Err(Error::new(
                    "syntax",
                    ppos,
                    format!("parameter `{}` has no type", pname),
                )
                .hint(format!(
                    "write `{}: Int` (or Str, Bool, List[T], Any)",
                    pname
                )));
            }
            let ty = self.ty()?;
            params.push(Param { name: pname, ty });
            if !self.eat_sym(",") {
                break;
            }
        }
        self.expect_sym(")")?;
        let ret = if self.eat_sym("->") {
            self.ty()?
        } else {
            Type::Unit
        };
        let effects = self.effects()?;
        let (mut requires, mut ensures) = (vec![], vec![]);
        loop {
            if self.eat_kw("requires") {
                requires.push(self.expr()?);
            } else if self.eat_kw("ensures") {
                ensures.push(self.expr()?);
            } else {
                break;
            }
        }
        if self.is_kw("uses") {
            return Err(self.err("`uses` must come before `requires` / `ensures`"));
        }
        let body = self.block()?;
        self.tparams.clear();
        Ok(Func {
            name,
            public: false,
            tparams,
            params,
            ret,
            effects,
            requires,
            ensures,
            body,
            pos,
        })
    }

    /// A capitalised name for a new type, variant or enum.
    fn type_name(&mut self, what: &str) -> Result<(String, Pos), Error> {
        let (name, npos) = self.ident()?;
        if BUILTIN_TYPES.contains(&name.as_str()) {
            return Err(Error::new(
                "syntax",
                npos,
                format!("`{}` is a built-in type and cannot be redefined", name),
            ));
        }
        if !name.starts_with(|c: char| c.is_ascii_uppercase()) {
            return Err(Error::new(
                "syntax",
                npos,
                format!("{} names start with a capital letter: `{}`", what, name),
            )
            .hint(format!(
                "write `{}{}`",
                name[..1].to_uppercase(),
                &name[1..]
            )));
        }
        Ok((name, npos))
    }

    /// `{ name: Type, ... }`, with commas or newlines between fields.
    fn field_decls(&mut self) -> Result<Vec<(String, Type, Pos)>, Error> {
        self.expect_sym("{")?;
        let mut fields = vec![];
        while !self.is_sym("}") {
            let (f, fpos) = self.ident()?;
            if !self.eat_sym(":") {
                return Err(
                    Error::new("syntax", fpos, format!("field `{}` has no type", f))
                        .hint(format!("write `{}: Int` (or another type)", f)),
                );
            }
            fields.push((f, self.ty()?, fpos));
            self.eat_sym(",");
        }
        self.bump();
        Ok(fields)
    }

    fn type_decl(&mut self) -> Result<TypeDecl, Error> {
        let pos = self.pos();
        self.bump();
        let (name, _) = self.type_name("type")?;
        let params = self.type_params()?;
        self.tparams = params.clone();
        let fields = self.field_decls()?;
        self.tparams.clear();
        Ok(TypeDecl {
            name,
            public: false,
            params,
            fields,
            pos,
        })
    }

    fn enum_decl(&mut self) -> Result<EnumDecl, Error> {
        let pos = self.pos();
        self.bump();
        let (name, _) = self.type_name("enum")?;
        let params = self.type_params()?;
        self.tparams = params.clone();
        self.expect_sym("{")?;
        let mut variants = vec![];
        while !self.is_sym("}") {
            let (vname, vpos) = self.type_name("variant")?;
            let fields = if self.is_sym("{") {
                self.field_decls()?
            } else {
                vec![]
            };
            variants.push(VariantDecl {
                name: vname,
                fields,
                pos: vpos,
            });
            self.eat_sym(",");
        }
        self.bump();
        self.tparams.clear();
        if variants.is_empty() {
            return Err(Error::new(
                "syntax",
                pos,
                format!("enum `{}` needs at least one variant", name),
            ));
        }
        Ok(EnumDecl {
            name,
            public: false,
            params,
            variants,
            pos,
        })
    }

    /// `{ field: expr, ... }` for record and variant values.
    fn field_values(&mut self) -> Result<Vec<(String, Expr, Pos)>, Error> {
        self.expect_sym("{")?;
        let mut fields = vec![];
        while !self.is_sym("}") {
            let (f, fpos) = self.ident()?;
            self.expect_sym(":")?;
            fields.push((f, self.expr()?, fpos));
            self.eat_sym(",");
        }
        self.bump();
        Ok(fields)
    }

    /// `module.name`, when the current token is an imported module's name followed by `.`.
    fn qualified(&mut self) -> Result<Option<(String, String, Pos)>, Error> {
        let Tok::Ident(m) = self.peek().clone() else {
            return Ok(None);
        };
        let dot_next = self.toks.get(self.i + 1).map(|t| &t.tok) == Some(&Tok::Sym("."));
        if !self.aliases.contains_key(&m) || !dot_next {
            return Ok(None);
        }
        let pos = self.pos();
        self.bump();
        self.bump();
        let (n, npos) = self.ident()?;
        if !self.aliases[&m].contains(&n) {
            return Err(Error::new(
                "name",
                npos,
                format!(
                    "module `{}` has no function, type or variant called `{}`",
                    m, n
                ),
            ));
        }
        Ok(Some((m, n, pos)))
    }

    fn pattern(&mut self) -> Result<Pattern, Error> {
        if let Some((m, n, pos)) = self.qualified()? {
            let name = format!("{}.{}", m, n);
            let mut binds = vec![];
            if self.eat_sym("{") {
                while !self.is_sym("}") {
                    let (f, fpos) = self.ident()?;
                    let var = if self.eat_sym(":") {
                        self.ident()?.0
                    } else {
                        f.clone()
                    };
                    binds.push((f, var, fpos));
                    self.eat_sym(",");
                }
                self.bump();
            }
            return Ok(Pattern::Variant(name, binds, pos));
        }
        let (n, pos) = self.ident()?;
        if n == "_" {
            return Ok(Pattern::Wildcard(pos));
        }
        let mut binds = vec![];
        if self.eat_sym("{") {
            while !self.is_sym("}") {
                let (f, fpos) = self.ident()?;
                let var = if self.eat_sym(":") {
                    self.ident()?.0
                } else {
                    f.clone()
                };
                binds.push((f, var, fpos));
                self.eat_sym(",");
            }
            self.bump();
        }
        Ok(Pattern::Variant(n, binds, pos))
    }

    fn test(&mut self) -> Result<Test, Error> {
        self.bump();
        let name = match self.peek().clone() {
            Tok::Str(s) => {
                self.bump();
                s
            }
            _ => {
                return Err(
                    self.err("expected a test name string, e.g. test \"adds numbers\" { ... }")
                )
            }
        };
        let effects = self.effects()?;
        let start = self.tok().start;
        let body = self.block()?;
        let text = self.src[start..self.toks[self.i - 1].end].to_string();
        Ok(Test {
            name,
            effects,
            body,
            text,
        })
    }

    fn effects(&mut self) -> Result<Vec<(String, Pos)>, Error> {
        let mut v = vec![];
        if self.eat_kw("uses") {
            loop {
                v.push(self.ident()?);
                if !self.eat_sym(",") {
                    break;
                }
            }
        }
        Ok(v)
    }

    /// Optional `[T, U]` after a declaration's name.
    fn type_params(&mut self) -> Result<Vec<String>, Error> {
        let mut params = vec![];
        if self.eat_sym("[") {
            while !self.is_sym("]") {
                let (p, ppos) = self.type_name("type parameter")?;
                if params.contains(&p) {
                    return Err(Error::new(
                        "syntax",
                        ppos,
                        format!("type parameter `{}` appears twice", p),
                    ));
                }
                params.push(p);
                if !self.eat_sym(",") {
                    break;
                }
            }
            self.expect_sym("]")?;
        }
        Ok(params)
    }

    /// Optional `[Int, Str]` after a generic type's name.
    fn type_args(&mut self) -> Result<Vec<Type>, Error> {
        let mut args = vec![];
        if self.eat_sym("[") {
            while !self.is_sym("]") {
                args.push(self.ty()?);
                if !self.eat_sym(",") {
                    break;
                }
            }
            self.expect_sym("]")?;
        }
        Ok(args)
    }

    fn ty(&mut self) -> Result<Type, Error> {
        if let Some((m, n, pos)) = self.qualified()? {
            let names = &self.aliases[&m];
            let name = format!("{}.{}", m, n);
            return if names.types.contains(&n) {
                Ok(Type::Record(name, self.type_args()?))
            } else if names.enums.contains(&n) {
                Ok(Type::Enum(name, self.type_args()?))
            } else {
                Err(Error::new(
                    "syntax",
                    pos,
                    format!("`{}` is not a type", name),
                ))
            };
        }
        let (n, pos) = self.ident()?;
        Ok(match n.as_str() {
            "Int" => Type::Int,
            "Str" => Type::Str,
            "Bool" => Type::Bool,
            "Unit" => Type::Unit,
            "Any" => Type::Any,
            "List" => {
                if self.eat_sym("[") {
                    let t = self.ty()?;
                    self.expect_sym("]")?;
                    Type::List(Box::new(t))
                } else {
                    Type::List(Box::new(Type::Any))
                }
            }
            _ if self.tparams.contains(&n) => Type::Param(n),
            _ if self.type_names.contains(&n) => Type::Record(n, self.type_args()?),
            _ if self.enum_names.contains(&n) => Type::Enum(n, self.type_args()?),
            _ => {
                return Err(
                    Error::new("syntax", pos, format!("unknown type `{}`", n)).hint(
                        "available types: Int, Str, Bool, Unit, List[T], Any, and your own `type`s",
                    ),
                )
            }
        })
    }

    fn block(&mut self) -> Result<Vec<Stmt>, Error> {
        self.expect_sym("{")?;
        let mut v = vec![];
        while !self.is_sym("}") {
            if matches!(self.peek(), Tok::Eof) {
                return Err(self.err("unexpected end of file: missing `}`"));
            }
            v.push(self.stmt()?);
        }
        self.bump();
        Ok(v)
    }

    fn stmt(&mut self) -> Result<Stmt, Error> {
        let pos = self.pos();
        if self.eat_kw("let") {
            let (n, _) = self.ident()?;
            let ann = if self.eat_sym(":") {
                Some(self.ty()?)
            } else {
                None
            };
            self.expect_sym("=")?;
            return Ok(Stmt::Let(n, ann, self.expr()?));
        }
        if self.eat_kw("if") {
            return self.if_rest();
        }
        if self.eat_kw("while") {
            let c = self.expr()?;
            return Ok(Stmt::While(c, self.block()?));
        }
        if self.eat_kw("for") {
            let (n, _) = self.ident()?;
            if !self.eat_kw("in") {
                return Err(self.err(format!("expected `in`, found {}", self.describe())));
            }
            let e = self.expr()?;
            return Ok(Stmt::For(n, e, self.block()?, pos));
        }
        if self.eat_kw("return") {
            let e = if self.is_sym("}") || self.tok().nl_before {
                None
            } else {
                Some(self.expr()?)
            };
            return Ok(Stmt::Return(e, pos));
        }
        if self.eat_kw("assert") {
            return Ok(Stmt::Assert(self.expr()?));
        }
        if self.eat_kw("match") {
            let e = self.expr()?;
            self.expect_sym("{")?;
            let mut arms = vec![];
            while !self.is_sym("}") {
                if matches!(self.peek(), Tok::Eof) {
                    return Err(self.err("unexpected end of file: missing `}`"));
                }
                let pattern = self.pattern()?;
                self.expect_sym("=>")?;
                let body = self.block()?;
                arms.push(Arm { pattern, body });
            }
            self.bump();
            return Ok(Stmt::Match(e, arms, pos));
        }
        let e = self.expr()?;
        if self.is_sym("=") {
            if let ExprKind::Var(n) = &e.kind {
                let n = n.clone();
                self.bump();
                return Ok(Stmt::Assign(n, self.expr()?, pos));
            }
            // `p.a.b = e`: collect the field path back to the variable.
            let mut path = vec![];
            let mut cur = &e;
            while let ExprKind::Field(target, f) = &cur.kind {
                path.push((f.clone(), cur.pos));
                cur = target;
            }
            if let (ExprKind::Var(n), false) = (&cur.kind, path.is_empty()) {
                let n = n.clone();
                path.reverse();
                self.bump();
                return Ok(Stmt::SetField(n, path, self.expr()?, pos));
            }
            return Err(
                self.err("can only assign to a variable or a field, like `x = ...` or `p.x = ...`")
            );
        }
        Ok(Stmt::Expr(e))
    }

    fn if_rest(&mut self) -> Result<Stmt, Error> {
        let c = self.expr()?;
        let then = self.block()?;
        let els = if self.eat_kw("else") {
            if self.eat_kw("if") {
                Some(vec![self.if_rest()?])
            } else {
                Some(self.block()?)
            }
        } else {
            None
        };
        Ok(Stmt::If(c, then, els))
    }

    // ---- expressions, lowest precedence first ----

    fn mk(&self, kind: ExprKind, start: usize) -> Expr {
        let t0 = &self.toks[start];
        let end = self.toks[self.i - 1].end;
        Expr {
            kind,
            pos: t0.pos,
            text: self.src[t0.start..end].to_string(),
        }
    }

    fn expr(&mut self) -> Result<Expr, Error> {
        self.or()
    }

    fn or(&mut self) -> Result<Expr, Error> {
        let s = self.i;
        let mut l = self.and()?;
        while self.eat_kw("or") {
            let r = self.and()?;
            l = self.mk(ExprKind::Binary(BinOp::Or, bx(l), bx(r)), s);
        }
        Ok(l)
    }

    fn and(&mut self) -> Result<Expr, Error> {
        let s = self.i;
        let mut l = self.not()?;
        while self.eat_kw("and") {
            let r = self.not()?;
            l = self.mk(ExprKind::Binary(BinOp::And, bx(l), bx(r)), s);
        }
        Ok(l)
    }

    fn not(&mut self) -> Result<Expr, Error> {
        let s = self.i;
        if self.eat_kw("not") {
            let e = self.not()?;
            return Ok(self.mk(ExprKind::Unary(UnOp::Not, bx(e)), s));
        }
        self.cmp()
    }

    fn op_of(&self, ops: &[(&str, BinOp)]) -> Option<BinOp> {
        ops.iter().find(|(s, _)| self.is_sym(s)).map(|(_, op)| *op)
    }

    fn cmp(&mut self) -> Result<Expr, Error> {
        use BinOp::*;
        let s = self.i;
        let l = self.add()?;
        let ops = [
            ("==", Eq),
            ("!=", Ne),
            ("<=", Le),
            (">=", Ge),
            ("<", Lt),
            (">", Gt),
        ];
        if let Some(op) = self.op_of(&ops) {
            self.bump();
            let r = self.add()?;
            return Ok(self.mk(ExprKind::Binary(op, bx(l), bx(r)), s));
        }
        Ok(l)
    }

    fn add(&mut self) -> Result<Expr, Error> {
        let s = self.i;
        let mut l = self.mul()?;
        while let Some(op) = self.op_of(&[("+", BinOp::Add), ("-", BinOp::Sub)]) {
            self.bump();
            let r = self.mul()?;
            l = self.mk(ExprKind::Binary(op, bx(l), bx(r)), s);
        }
        Ok(l)
    }

    fn mul(&mut self) -> Result<Expr, Error> {
        let s = self.i;
        let mut l = self.unary()?;
        while let Some(op) = self.op_of(&[("*", BinOp::Mul), ("/", BinOp::Div), ("%", BinOp::Mod)])
        {
            self.bump();
            let r = self.unary()?;
            l = self.mk(ExprKind::Binary(op, bx(l), bx(r)), s);
        }
        Ok(l)
    }

    fn unary(&mut self) -> Result<Expr, Error> {
        let s = self.i;
        if self.eat_sym("-") {
            let e = self.unary()?;
            return Ok(self.mk(ExprKind::Unary(UnOp::Neg, bx(e)), s));
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Result<Expr, Error> {
        let s = self.i;
        let mut e = self.primary()?;
        loop {
            if self.is_sym("[") && !self.tok().nl_before {
                self.bump();
                let idx = self.expr()?;
                self.expect_sym("]")?;
                e = self.mk(ExprKind::Index(bx(e), bx(idx)), s);
            } else if self.eat_sym(".") {
                let (f, _) = self.ident()?;
                e = self.mk(ExprKind::Field(bx(e), f), s);
            } else {
                return Ok(e);
            }
        }
    }

    fn primary(&mut self) -> Result<Expr, Error> {
        let s = self.i;
        if let Some((m, n, pos)) = self.qualified()? {
            let names = self.aliases[&m].clone();
            let name = format!("{}.{}", m, n);
            if self.is_sym("(") && !self.tok().nl_before && names.funcs.contains(&n) {
                self.bump();
                let mut args = vec![];
                while !self.is_sym(")") {
                    args.push(self.expr()?);
                    if !self.eat_sym(",") {
                        break;
                    }
                }
                self.expect_sym(")")?;
                return Ok(self.mk(ExprKind::Call(name, args), s));
            }
            if names.types.contains(&n) && self.is_sym("{") {
                let fields = self.field_values()?;
                return Ok(self.mk(ExprKind::Record(name, fields), s));
            }
            if names.variants_with_fields.contains(&n) && self.is_sym("{") {
                let fields = self.field_values()?;
                return Ok(self.mk(ExprKind::Variant(name, fields), s));
            }
            if names.variants.contains(&n) {
                return Ok(self.mk(ExprKind::Variant(name, vec![]), s));
            }
            let hint = if names.funcs.contains(&n) {
                format!("call it: `{}(...)`", name)
            } else {
                format!("create a value with `{} {{ ... }}`", name)
            };
            return Err(Error::new("syntax", pos, format!("`{}` is not a value", name)).hint(hint));
        }
        let t = self.bump();
        match t.tok {
            Tok::Int(n) => Ok(self.mk(ExprKind::Int(n), s)),
            Tok::Str(x) => Ok(self.mk(ExprKind::Str(x), s)),
            Tok::Ident(x) if x == "true" || x == "false" => {
                Ok(self.mk(ExprKind::Bool(x == "true"), s))
            }
            Tok::Ident(x) if !KEYWORDS.contains(&x.as_str()) => {
                if self.is_sym("(") && !self.tok().nl_before {
                    self.bump();
                    let mut args = vec![];
                    while !self.is_sym(")") {
                        args.push(self.expr()?);
                        if !self.eat_sym(",") {
                            break;
                        }
                    }
                    self.expect_sym(")")?;
                    Ok(self.mk(ExprKind::Call(x, args), s))
                } else if self.type_names.contains(&x) && self.is_sym("{") {
                    let fields = self.field_values()?;
                    Ok(self.mk(ExprKind::Record(x, fields), s))
                } else if self.variants_with_fields.contains(&x) && self.is_sym("{") {
                    let fields = self.field_values()?;
                    Ok(self.mk(ExprKind::Variant(x, fields), s))
                } else if self.variant_names.contains(&x) {
                    Ok(self.mk(ExprKind::Variant(x, vec![]), s))
                } else {
                    Ok(self.mk(ExprKind::Var(x), s))
                }
            }
            Tok::Sym("(") => {
                let e = self.expr()?;
                self.expect_sym(")")?;
                Ok(e)
            }
            Tok::Sym("[") => {
                let mut items = vec![];
                while !self.is_sym("]") {
                    items.push(self.expr()?);
                    if !self.eat_sym(",") {
                        break;
                    }
                }
                self.expect_sym("]")?;
                Ok(self.mk(ExprKind::List(items), s))
            }
            _ => {
                self.i = s;
                Err(self.err(format!("expected an expression, found {}", self.describe())))
            }
        }
    }
}
