#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
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
    pub fn render(&self, file: &str, src: &str) -> String {
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
    pub fn to_json(&self, file: &str) -> String {
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
