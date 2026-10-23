use crate::ast::*;
use crate::error::{Error, Pos};
use crate::lexer::{lex, Tok, Token};

const KEYWORDS: [&str; 18] = [
    "fn", "let", "if", "else", "while", "for", "in", "return", "true", "false", "uses", "requires",
    "ensures", "test", "assert", "and", "or", "not",
];

pub fn parse(src: &str) -> Result<Program, Error> {
    let mut p = Parser {
        toks: lex(src)?,
        i: 0,
        src,
    };
    let mut prog = Program {
        funcs: vec![],
        tests: vec![],
    };
    loop {
        if matches!(p.peek(), Tok::Eof) {
            break;
        } else if p.is_kw("fn") {
            prog.funcs.push(p.func()?);
        } else if p.is_kw("test") {
            prog.tests.push(p.test()?);
        } else {
            return Err(p.err(format!(
                "expected `fn` or `test` at top level, found {}",
                p.describe()
            )));
        }
    }
    Ok(prog)
}

struct Parser<'a> {
    toks: Vec<Token>,
    i: usize,
    src: &'a str,
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
        Ok(Func {
            name,
            params,
            ret,
            effects,
            requires,
            ensures,
            body,
            pos,
        })
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
        let body = self.block()?;
        Ok(Test {
            name,
            effects,
            body,
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

    fn ty(&mut self) -> Result<Type, Error> {
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
            _ => {
                return Err(Error::new("syntax", pos, format!("unknown type `{}`", n))
                    .hint("available types: Int, Str, Bool, Unit, List[T], Any"))
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
        let e = self.expr()?;
        if self.is_sym("=") {
            if let ExprKind::Var(n) = &e.kind {
                let n = n.clone();
                self.bump();
                return Ok(Stmt::Assign(n, self.expr()?, pos));
            }
            return Err(self.err("can only assign to a variable name"));
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
        while self.is_sym("[") && !self.tok().nl_before {
            self.bump();
            let idx = self.expr()?;
            self.expect_sym("]")?;
            e = self.mk(ExprKind::Index(bx(e), bx(idx)), s);
        }
        Ok(e)
    }

    fn primary(&mut self) -> Result<Expr, Error> {
        let s = self.i;
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
