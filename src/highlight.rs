use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tok {
    Normal,
    Keyword,
    Type,
    Str,
    Comment,
    Number,
    Func,
    Macro,
}

pub struct LangDef {
    pub name: &'static str,
    pub exts: &'static [&'static str],
    pub keywords: &'static [&'static str],
    pub types: &'static [&'static str],
    pub line_comment: &'static [&'static str],
    pub block: Option<(&'static str, &'static str)>,
    pub quotes: &'static [char],
    pub macros: bool,
    pub caps_types: bool,
    pub plain: bool,
}

static PLAIN: LangDef = LangDef {
    name: "Metin",
    exts: &[],
    keywords: &[],
    types: &[],
    line_comment: &[],
    block: None,
    quotes: &[],
    macros: false,
    caps_types: false,
    plain: true,
};

static RUST: LangDef = LangDef {
    name: "Rust",
    exts: &["rs"],
    keywords: &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false",
        "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
        "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while",
        "yield",
    ],
    types: &[
        "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64",
        "bool", "char", "str", "String", "Vec", "Option", "Result", "Box", "Some", "None", "Ok", "Err",
    ],
    line_comment: &["//"],
    block: Some(("/*", "*/")),
    quotes: &['"', '\''],
    macros: true,
    caps_types: true,
    plain: false,
};

static C: LangDef = LangDef {
    name: "C/C++",
    exts: &["c", "h", "cpp", "cc", "cxx", "hpp", "hh", "hxx"],
    keywords: &[
        "auto", "break", "case", "const", "continue", "default", "do", "else", "enum", "extern", "for", "goto",
        "if", "inline", "register", "return", "sizeof", "static", "struct", "switch", "typedef", "union",
        "volatile", "while", "class", "namespace", "template", "typename", "public", "private", "protected",
        "virtual", "override", "new", "delete", "this", "using", "nullptr", "true", "false", "include", "define",
        "ifdef", "ifndef", "endif", "pragma", "constexpr", "noexcept", "operator", "friend", "try", "catch",
        "throw",
    ],
    types: &[
        "int", "char", "float", "double", "long", "short", "signed", "unsigned", "void", "bool", "size_t",
        "uint8_t", "uint16_t", "uint32_t", "uint64_t", "int8_t", "int16_t", "int32_t", "int64_t", "std",
    ],
    line_comment: &["//"],
    block: Some(("/*", "*/")),
    quotes: &['"', '\''],
    macros: false,
    caps_types: true,
    plain: false,
};

static PYTHON: LangDef = LangDef {
    name: "Python",
    exts: &["py", "pyw", "pyi"],
    keywords: &[
        "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else",
        "except", "False", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "None",
        "nonlocal", "not", "or", "pass", "raise", "return", "True", "try", "while", "with", "yield", "self",
        "match", "case",
    ],
    types: &["int", "str", "float", "bool", "list", "dict", "set", "tuple", "bytes", "object"],
    line_comment: &["#"],
    block: None,
    quotes: &['"', '\''],
    macros: false,
    caps_types: true,
    plain: false,
};

static JS: LangDef = LangDef {
    name: "JS/TS",
    exts: &["js", "mjs", "cjs", "jsx", "ts", "tsx", "mts"],
    keywords: &[
        "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do", "else",
        "export", "extends", "false", "finally", "for", "function", "if", "import", "in", "instanceof", "let",
        "new", "null", "return", "super", "switch", "this", "throw", "true", "try", "typeof", "undefined", "var",
        "void", "while", "with", "yield", "async", "await", "of", "interface", "type", "enum", "implements",
        "private", "public", "protected", "readonly", "static", "as", "from",
    ],
    types: &["string", "number", "boolean", "any", "unknown", "never", "object"],
    line_comment: &["//"],
    block: Some(("/*", "*/")),
    quotes: &['"', '\'', '`'],
    macros: false,
    caps_types: true,
    plain: false,
};

static GO: LangDef = LangDef {
    name: "Go",
    exts: &["go"],
    keywords: &[
        "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "for", "func",
        "go", "goto", "if", "import", "interface", "map", "package", "range", "return", "select", "struct",
        "switch", "type", "var", "nil", "true", "false",
    ],
    types: &[
        "int", "int8", "int16", "int32", "int64", "uint", "uint8", "uint16", "uint32", "uint64", "float32",
        "float64", "string", "bool", "byte", "rune", "error", "any",
    ],
    line_comment: &["//"],
    block: Some(("/*", "*/")),
    quotes: &['"', '\'', '`'],
    macros: false,
    caps_types: false,
    plain: false,
};

static SHELL: LangDef = LangDef {
    name: "Shell",
    exts: &["sh", "bash", "zsh", "fish"],
    keywords: &[
        "if", "then", "else", "elif", "fi", "for", "while", "do", "done", "case", "esac", "function", "in",
        "return", "local", "export", "echo", "exit", "source",
    ],
    types: &[],
    line_comment: &["#"],
    block: None,
    quotes: &['"', '\''],
    macros: false,
    caps_types: false,
    plain: false,
};

static TOML: LangDef = LangDef {
    name: "TOML",
    exts: &["toml", "ini", "cfg", "conf", "yaml", "yml"],
    keywords: &["true", "false"],
    types: &[],
    line_comment: &["#", ";"],
    block: None,
    quotes: &['"', '\''],
    macros: false,
    caps_types: false,
    plain: false,
};

static JSON: LangDef = LangDef {
    name: "JSON",
    exts: &["json", "jsonc"],
    keywords: &["true", "false", "null"],
    types: &[],
    line_comment: &[],
    block: None,
    quotes: &['"'],
    macros: false,
    caps_types: false,
    plain: false,
};

static LUA: LangDef = LangDef {
    name: "Lua",
    exts: &["lua"],
    keywords: &[
        "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in", "local", "nil",
        "not", "or", "repeat", "return", "then", "true", "until", "while",
    ],
    types: &[],
    line_comment: &["--"],
    block: Some(("--[[", "]]")),
    quotes: &['"', '\''],
    macros: false,
    caps_types: false,
    plain: false,
};

static LANGS: &[&LangDef] = &[&RUST, &C, &PYTHON, &JS, &GO, &SHELL, &TOML, &JSON, &LUA];

pub fn plain() -> &'static LangDef {
    &PLAIN
}

pub fn detect(path: &Path) -> &'static LangDef {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    LANGS.iter().copied().find(|l| l.exts.contains(&ext.as_str())).unwrap_or(&PLAIN)
}

fn starts_with(line: &[char], i: usize, pat: &str) -> bool {
    let mut j = i;
    for p in pat.chars() {
        if j >= line.len() || line[j] != p {
            return false;
        }
        j += 1;
    }
    true
}

fn is_rust_char_lit(line: &[char], i: usize) -> bool {
    line.get(i + 1) == Some(&'\\') || line.get(i + 2) == Some(&'\'')
}

pub fn highlight_line(lang: &LangDef, line: &[char], in_block: &mut bool) -> Vec<Tok> {
    let n = line.len();
    let mut out = vec![Tok::Normal; n];
    if lang.plain {
        return out;
    }
    let mut i = 0;
    while i < n {
        if *in_block {
            let end = lang.block.map(|b| b.1).unwrap_or("*/");
            let start = i;
            let mut closed = false;
            while i < n {
                if starts_with(line, i, end) {
                    i += end.chars().count();
                    closed = true;
                    break;
                }
                i += 1;
            }
            for t in &mut out[start..i.min(n)] {
                *t = Tok::Comment;
            }
            if closed {
                *in_block = false;
            }
            continue;
        }
        let c = line[i];
        if let Some((s, _)) = lang.block {
            if starts_with(line, i, s) {
                *in_block = true;
                let l = s.chars().count();
                for t in &mut out[i..(i + l).min(n)] {
                    *t = Tok::Comment;
                }
                i += l;
                continue;
            }
        }
        if lang.line_comment.iter().any(|lc| starts_with(line, i, lc)) {
            for t in &mut out[i..] {
                *t = Tok::Comment;
            }
            break;
        }
        if lang.quotes.contains(&c) {
            if c == '\'' && lang.macros && !is_rust_char_lit(line, i) {
                let start = i;
                i += 1;
                while i < n && (line[i].is_alphanumeric() || line[i] == '_') {
                    i += 1;
                }
                for t in &mut out[start..i] {
                    *t = Tok::Macro;
                }
                continue;
            }
            let start = i;
            i += 1;
            while i < n {
                if line[i] == '\\' {
                    i += 2;
                    continue;
                }
                if line[i] == c {
                    i += 1;
                    break;
                }
                i += 1;
            }
            let end = i.min(n);
            for t in &mut out[start..end] {
                *t = Tok::Str;
            }
            i = end;
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            while i < n {
                let ch = line[i];
                if ch == '.' {
                    if i + 1 >= n || !line[i + 1].is_ascii_digit() {
                        break;
                    }
                } else if !(ch.is_ascii_alphanumeric() || ch == '_') {
                    break;
                }
                i += 1;
            }
            for t in &mut out[start..i] {
                *t = Tok::Number;
            }
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < n && (line[i].is_alphanumeric() || line[i] == '_') {
                i += 1;
            }
            let word: String = line[start..i].iter().collect();
            let w = word.as_str();
            let next_nb = line[i..].iter().find(|c| !c.is_whitespace()).copied();
            let tok = if lang.keywords.contains(&w) {
                Tok::Keyword
            } else if lang.types.contains(&w) {
                Tok::Type
            } else if lang.macros && line.get(i) == Some(&'!') && line.get(i + 1) != Some(&'=') {
                i += 1;
                Tok::Macro
            } else if next_nb == Some('(') {
                Tok::Func
            } else if lang.caps_types && c.is_uppercase() {
                Tok::Type
            } else {
                Tok::Normal
            };
            for t in &mut out[start..i] {
                *t = tok;
            }
            continue;
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_line() {
        let line: Vec<char> = "let x: &'a str = \"hi\"; // c".chars().collect();
        let mut b = false;
        let t = highlight_line(&RUST, &line, &mut b);
        assert_eq!(t[0], Tok::Keyword);
        assert_eq!(t[8], Tok::Macro);
        assert_eq!(t[18], Tok::Str);
        assert_eq!(*t.last().unwrap(), Tok::Comment);
    }

    #[test]
    fn block() {
        let mut b = false;
        let l1: Vec<char> = "a /* b".chars().collect();
        let l2: Vec<char> = "c */ d".chars().collect();
        highlight_line(&RUST, &l1, &mut b);
        assert!(b);
        let t = highlight_line(&RUST, &l2, &mut b);
        assert!(!b);
        assert_eq!(t[0], Tok::Comment);
        assert_eq!(t[5], Tok::Normal);
    }
}
