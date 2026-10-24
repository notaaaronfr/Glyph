#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
    /// Index into the program's `Sources` (0 is the file that was run).
    pub file: usize,
}

impl Pos {
    pub fn new(line: usize, col: usize, file: usize) -> Self {
        Pos { line, col, file }
    }
}

/// The source files of a program: (path as shown to the user, contents).
#[derive(Debug, Clone, Default)]
pub struct Sources(pub Vec<(String, String)>);

impl Sources {
    pub fn path(&self, file: usize) -> &str {
        self.0.get(file).map_or("?", |f| f.0.as_str())
    }

    pub fn src(&self, file: usize) -> &str {
        self.0.get(file).map_or("", |f| f.1.as_str())
    }
}

/// Every diagnostic in Glyph: syntax, name, effect, type, contract, runtime, assertion.
#[derive(Debug, Clone)]
pub struct Error {
    pub kind: &'static str,
    pub msg: String,
    pub pos: Pos,
    pub hint: Option<String>,
}

impl Error {
    pub fn new(kind: &'static str, pos: Pos, msg: impl Into<String>) -> Self {
        Error {
            kind,
            msg: msg.into(),
            pos,
            hint: None,
        }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Human-readable diagnostic with the offending source line and a caret.
    pub fn render(&self, sources: &Sources) -> String {
        let (file, src) = (sources.path(self.pos.file), sources.src(self.pos.file));
        let mut s = format!(
            "{}:{}:{}: {} error: {}\n",
            file, self.pos.line, self.pos.col, self.kind, self.msg
        );
        if let Some(line) = src.lines().nth(self.pos.line.wrapping_sub(1)) {
            let n = self.pos.line.to_string();
            s += &format!(
                "{} | {}\n{} | {}^\n",
                n,
                line,
                " ".repeat(n.len()),
                " ".repeat(self.pos.col.saturating_sub(1))
            );
        }
        if let Some(h) = &self.hint {
            s += &format!("  hint: {}\n", h);
        }
        s
    }

    /// One JSON object per line, for AI agents and editors.
    pub fn to_json(&self, sources: &Sources) -> String {
        let file = sources.path(self.pos.file);
        format!(
            "{{\"file\":{},\"line\":{},\"col\":{},\"kind\":{},\"message\":{},\"hint\":{}}}",
            json_str(file),
            self.pos.line,
            self.pos.col,
            json_str(self.kind),
            json_str(&self.msg),
            self.hint
                .as_deref()
                .map(json_str)
                .unwrap_or_else(|| "null".into())
        )
    }
}

pub fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            '\n' => out += "\\n",
            '\t' => out += "\\t",
            c if (c as u32) < 0x20 => out += &format!("\\u{:04x}", c as u32),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
