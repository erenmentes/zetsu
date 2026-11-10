use std::path::{Path, PathBuf};
use tree_sitter::{Node, Parser};
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Rust,
    Python,
    Go,
    C,
    Cpp,
    Js,
    Ts,
    Tsx,
}

pub fn lang_for(path: &Path) -> Option<Lang> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "rs" => Lang::Rust,
        "py" | "pyw" => Lang::Python,
        "go" => Lang::Go,
        "c" | "h" => Lang::C,
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => Lang::Cpp,
        "js" | "mjs" | "cjs" | "jsx" => Lang::Js,
        "ts" | "mts" | "cts" => Lang::Ts,
        "tsx" => Lang::Tsx,
        _ => return None,
    })
}

fn language(l: Lang) -> tree_sitter::Language {
    match l {
        Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
        Lang::Python => tree_sitter_python::LANGUAGE.into(),
        Lang::Go => tree_sitter_go::LANGUAGE.into(),
        Lang::C => tree_sitter_c::LANGUAGE.into(),
        Lang::Cpp => tree_sitter_cpp::LANGUAGE.into(),
        Lang::Js => tree_sitter_javascript::LANGUAGE.into(),
        Lang::Ts => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JumpKind {
    Return,
    Break,
    Continue,
    Throw,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub line: usize,
    pub end: usize,
    pub text: String,
}

#[derive(Clone, Debug)]
pub enum Flow {
    Seq(Vec<Flow>),
    Stmt(Vec<Item>),
    If {
        cond: String,
        line: usize,
        end: usize,
        then: Box<Flow>,
        els: Option<Box<Flow>>,
    },
    Loop {
        head: String,
        line: usize,
        end: usize,
        body: Box<Flow>,
    },
    Switch {
        head: String,
        line: usize,
        end: usize,
        arms: Vec<(String, Flow)>,
    },
    Jump {
        kind: JumpKind,
        text: String,
        line: usize,
        end: usize,
    },
}

#[derive(Clone, Debug)]
pub struct FnInfo {
    pub name: String,
    pub start: usize,
    pub end: usize,
    pub flow: Flow,
    pub branches: usize,
}

const IF: &[&str] = &["if_expression", "if_statement"];
const LOOP: &[&str] = &[
    "loop_expression",
    "while_expression",
    "for_expression",
    "for_statement",
    "while_statement",
    "do_statement",
    "for_in_statement",
    "for_of_statement",
    "for_range_loop",
];
const SWITCH: &[&str] = &[
    "match_expression",
    "match_statement",
    "switch_statement",
    "expression_switch_statement",
    "type_switch_statement",
    "select_statement",
];
const ARM: &[&str] = &[
    "match_arm",
    "case_clause",
    "case_statement",
    "switch_case",
    "switch_default",
    "expression_case",
    "default_case",
    "type_case",
    "communication_case",
];
const BLOCK: &[&str] = &["block", "compound_statement", "statement_block"];
const SKIP: &[&str] = &[
    "comment",
    "line_comment",
    "block_comment",
    "attribute_item",
    "inner_attribute_item",
    "empty_statement",
];
const FUNCS: &[&str] = &[
    "function_item",
    "function_definition",
    "function_declaration",
    "method_declaration",
    "generator_function_declaration",
    "method_definition",
];

fn is_control(k: &str) -> bool {
    IF.contains(&k)
        || LOOP.contains(&k)
        || SWITCH.contains(&k)
        || jump_kind(k).is_some()
        || k == "try_statement"
        || k == "unsafe_block"
        || BLOCK.contains(&k)
}

fn jump_kind(k: &str) -> Option<JumpKind> {
    Some(match k {
        "return_expression" | "return_statement" => JumpKind::Return,
        "break_expression" | "break_statement" => JumpKind::Break,
        "continue_expression" | "continue_statement" => JumpKind::Continue,
        "raise_statement" | "throw_statement" => JumpKind::Throw,
        _ => return None,
    })
}

pub fn clean(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            space = !out.is_empty();
        } else {
            if space {
                out.push(' ');
                space = false;
            }
            out.push(c);
        }
    }
    while out.ends_with('{') || out.ends_with(':') || out.ends_with(' ') {
        out.pop();
    }
    out
}

struct P<'a> {
    src: &'a [u8],
    lang: Lang,
}

impl<'a> P<'a> {
    fn text(&self, n: Node) -> &'a str {
        std::str::from_utf8(&self.src[n.start_byte()..n.end_byte()]).unwrap_or("")
    }

    fn first_line(&self, n: Node) -> String {
        clean(self.text(n).lines().next().unwrap_or(""))
    }

    fn header(&self, n: Node, until: Option<Node>) -> String {
        let end = until.map_or(n.end_byte(), |u| u.start_byte()).max(n.start_byte());
        let s = std::str::from_utf8(&self.src[n.start_byte()..end]).unwrap_or("");
        clean(s)
    }

    fn flat(&self, n: Node) -> String {
        let s = clean(self.text(n));
        let t = s.trim();
        if t.starts_with('(')
            && t.ends_with(')')
            && matches!(self.lang, Lang::C | Lang::Cpp | Lang::Js | Lang::Ts | Lang::Tsx)
        {
            t[1..t.len() - 1].trim().to_string()
        } else {
            t.to_string()
        }
    }

    fn item(&self, n: Node) -> Item {
        Item {
            line: n.start_position().row,
            end: n.end_position().row,
            text: self.first_line(n),
        }
    }

    fn push_stmt(&self, n: Node, out: &mut Vec<Flow>) {
        let it = self.item(n);
        if let Some(Flow::Stmt(v)) = out.last_mut() {
            v.push(it);
        } else {
            out.push(Flow::Stmt(vec![it]));
        }
    }

    fn body(&self, n: Option<Node>) -> Flow {
        let mut out = Vec::new();
        if let Some(n) = n {
            if BLOCK.contains(&n.kind()) {
                self.children(n, &mut out);
            } else {
                self.stmt(n, &mut out);
            }
        }
        Flow::Seq(out)
    }

    fn children(&self, n: Node, out: &mut Vec<Flow>) {
        let mut c = n.walk();
        let kids: Vec<Node> = n.named_children(&mut c).collect();
        for k in kids {
            self.stmt(k, out);
        }
    }

    fn stmt(&self, n: Node, out: &mut Vec<Flow>) {
        let k = n.kind();
        if SKIP.contains(&k) || !n.is_named() {
            return;
        }
        if k == "expression_statement" {
            if n.named_child_count() == 1 {
                if let Some(c) = n.named_child(0) {
                    if is_control(c.kind()) {
                        return self.stmt(c, out);
                    }
                }
            }
            return self.push_stmt(n, out);
        }
        if BLOCK.contains(&k) {
            return self.children(n, out);
        }
        if k == "unsafe_block" || k == "labeled_statement" {
            let mut c = n.walk();
            let last = n.named_children(&mut c).last();
            if let Some(b) = last {
                return self.stmt(b, out);
            }
        }
        if k == "let_declaration" {
            if let Some(v) = n.child_by_field_name("value") {
                if IF.contains(&v.kind()) || SWITCH.contains(&v.kind()) {
                    self.stmt(v, out);
                    return self.push_stmt(n, out);
                }
            }
        }
        if IF.contains(&k) {
            return out.push(self.if_flow(n));
        }
        if LOOP.contains(&k) {
            return out.push(self.loop_flow(n));
        }
        if SWITCH.contains(&k) {
            return out.push(self.switch_flow(n));
        }
        if k == "try_statement" {
            return self.try_flow(n, out);
        }
        if k == "with_statement" {
            let b = n.child_by_field_name("body");
            out.push(Flow::Stmt(vec![Item {
                line: n.start_position().row,
                end: b.map_or(n.end_position().row, |b| b.start_position().row),
                text: self.header(n, b),
            }]));
            if let Flow::Seq(v) = self.body(b) {
                out.extend(v);
            }
            return;
        }
        if let Some(j) = jump_kind(k) {
            return out.push(Flow::Jump {
                kind: j,
                text: self.first_line(n),
                line: n.start_position().row,
                end: n.end_position().row,
            });
        }
        self.push_stmt(n, out)
    }

    fn if_flow(&self, n: Node) -> Flow {
        let cons = n.child_by_field_name("consequence");
        let cond = match n.child_by_field_name("condition") {
            Some(c) => self.flat(c),
            None => self.header(n, cons),
        };
        let mut c = n.walk();
        let alts: Vec<Node> = n.children_by_field_name("alternative", &mut c).collect();
        Flow::If {
            cond,
            line: n.start_position().row,
            end: hdr_end(n, cons),
            then: Box::new(self.body(cons)),
            els: self.else_chain(&alts),
        }
    }

    fn else_chain(&self, alts: &[Node]) -> Option<Box<Flow>> {
        let first = *alts.first()?;
        let f = match first.kind() {
            "elif_clause" => {
                let cons = first.child_by_field_name("consequence");
                Flow::If {
                    cond: first
                        .child_by_field_name("condition")
                        .map_or_else(|| self.header(first, cons), |c| self.flat(c)),
                    line: first.start_position().row,
                    end: hdr_end(first, cons),
                    then: Box::new(self.body(cons)),
                    els: self.else_chain(&alts[1..]),
                }
            }
            "else_clause" => {
                let inner = first.child_by_field_name("body").or_else(|| {
                    let mut c = first.walk();
                    let r = first.named_children(&mut c).find(|x| !SKIP.contains(&x.kind()));
                    r
                });
                match inner {
                    Some(i) if IF.contains(&i.kind()) => self.if_flow(i),
                    other => self.body(other),
                }
            }
            k if IF.contains(&k) => self.if_flow(first),
            _ => self.body(Some(first)),
        };
        Some(Box::new(f))
    }

    fn loop_flow(&self, n: Node) -> Flow {
        let body = n.child_by_field_name("body");
        let mut head = self.header(n, body);
        if n.kind() == "do_statement" {
            let cond = n
                .child_by_field_name("condition")
                .map(|c| self.flat(c))
                .unwrap_or_default();
            head = format!("do … while {}", cond);
        }
        if head.is_empty() {
            head = n.kind().split('_').next().unwrap_or("loop").to_string();
        }
        Flow::Loop {
            head,
            line: n.start_position().row,
            end: hdr_end(n, body),
            body: Box::new(self.body(body)),
        }
    }

    fn arm(&self, a: Node) -> (String, Flow) {
        if a.kind() == "match_arm" {
            let label = a
                .child_by_field_name("pattern")
                .map(|p| self.flat(p))
                .unwrap_or_default();
            let mut f = self.body(a.child_by_field_name("value"));
            if let Flow::Seq(v) = &f {
                if v.is_empty() {
                    f = Flow::Seq(vec![Flow::Stmt(vec![self.item(a)])]);
                }
            }
            return (label, f);
        }
        if let Some(cons) = a.child_by_field_name("consequence") {
            let label = self.header(a, Some(cons));
            return (label, self.body(Some(cons)));
        }
        let mut c = a.walk();
        let kids: Vec<Node> = a.children(&mut c).collect();
        let colon = kids.iter().position(|k| k.kind() == ":");
        let (label_end, rest) = match colon {
            Some(i) => (kids[i].start_byte(), &kids[i + 1..]),
            None => (a.end_byte(), &kids[kids.len()..]),
        };
        let label = clean(std::str::from_utf8(&self.src[a.start_byte()..label_end]).unwrap_or(""));
        let mut out = Vec::new();
        for k in rest {
            self.stmt(*k, &mut out);
        }
        if matches!(
            out.last(),
            Some(Flow::Jump {
                kind: JumpKind::Break,
                ..
            })
        ) {
            out.pop();
        }
        (label, Flow::Seq(out))
    }

    fn switch_flow(&self, n: Node) -> Flow {
        let body = n.child_by_field_name("body");
        let container = body.unwrap_or(n);
        let mut c = container.walk();
        let arm_nodes: Vec<Node> = container
            .named_children(&mut c)
            .filter(|x| ARM.contains(&x.kind()))
            .collect();
        let until = body.or_else(|| arm_nodes.first().copied());
        let arms = arm_nodes.iter().map(|a| self.arm(*a)).collect();
        Flow::Switch {
            head: self.header(n, until),
            line: n.start_position().row,
            end: hdr_end(n, until),
            arms,
        }
    }

    fn try_flow(&self, n: Node, out: &mut Vec<Flow>) {
        let body = n.child_by_field_name("body");
        let mut arms = vec![("try".to_string(), self.body(body))];
        let mut fin = None;
        let mut c = n.walk();
        let kids: Vec<Node> = n.named_children(&mut c).collect();
        for k in kids {
            if Some(k) == body {
                continue;
            }
            let blk = k.child_by_field_name("body").or_else(|| {
                let mut c2 = k.walk();
                let r = k.named_children(&mut c2).filter(|x| BLOCK.contains(&x.kind())).last();
                r
            });
            match k.kind() {
                "finally_clause" => fin = Some(self.body(blk)),
                "except_clause" | "except_group_clause" | "catch_clause" | "else_clause" => {
                    arms.push((self.header(k, blk), self.body(blk)))
                }
                _ => {}
            }
        }
        out.push(Flow::Switch {
            head: "try".into(),
            line: n.start_position().row,
            end: hdr_end(n, body),
            arms,
        });
        if let Some(Flow::Seq(v)) = fin {
            out.extend(v);
        }
    }

    fn c_name(&self, d: Node) -> String {
        match d.kind() {
            "function_declarator" | "pointer_declarator" | "reference_declarator" | "parenthesized_declarator" => {
                match d.child_by_field_name("declarator") {
                    Some(i) => self.c_name(i),
                    None => {
                        let mut c = d.walk();
                        let r = d
                            .named_children(&mut c)
                            .next()
                            .map(|x| self.c_name(x))
                            .unwrap_or_default();
                        r
                    }
                }
            }
            _ => self.text(d).to_string(),
        }
    }

    fn owner(&self, n: Node) -> Option<String> {
        if n.kind() == "method_declaration" {
            let r = n.child_by_field_name("receiver")?;
            let t = self.text(r);
            let name = t
                .trim_matches(|c| c == '(' || c == ')')
                .split_whitespace()
                .last()?
                .trim_start_matches('*')
                .to_string();
            return Some(name);
        }
        let mut p = n.parent();
        while let Some(a) = p {
            match a.kind() {
                "impl_item" => return a.child_by_field_name("type").map(|t| self.text(t).to_string()),
                "trait_item" | "class_definition" | "class_declaration" | "class" | "class_specifier"
                | "struct_specifier" => {
                    return a.child_by_field_name("name").map(|t| self.text(t).to_string());
                }
                k if FUNCS.contains(&k) => return None,
                _ => {}
            }
            p = a.parent();
        }
        None
    }

    fn fn_name(&self, n: Node) -> String {
        let base = if let Some(nm) = n.child_by_field_name("name") {
            self.text(nm).to_string()
        } else if let Some(d) = n.child_by_field_name("declarator") {
            self.c_name(d)
        } else if let Some(p) = n.parent().filter(|p| p.kind() == "variable_declarator") {
            p.child_by_field_name("name")
                .map(|x| self.text(x).to_string())
                .unwrap_or_default()
        } else if let Some(p) = n.parent().filter(|p| p.kind() == "pair") {
            p.child_by_field_name("key")
                .map(|x| self.text(x).to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let base = if base.is_empty() { "<anon>".to_string() } else { base };
        if base.contains("::") {
            return base;
        }
        match self.owner(n) {
            Some(o) => {
                let sep = if matches!(self.lang, Lang::Rust | Lang::Cpp) {
                    "::"
                } else {
                    "."
                };
                format!("{}{}{}", o, sep, base)
            }
            None => base,
        }
    }

    fn is_fn(&self, n: Node) -> bool {
        let k = n.kind();
        if FUNCS.contains(&k) {
            return n.child_by_field_name("body").is_some();
        }
        if matches!(k, "arrow_function" | "function_expression" | "function") {
            return n
                .parent()
                .is_some_and(|p| matches!(p.kind(), "variable_declarator" | "pair" | "public_field_definition"));
        }
        false
    }

    fn collect(&self, root: Node, out: &mut Vec<FnInfo>) {
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            if self.is_fn(n) {
                let flow = self.body(n.child_by_field_name("body"));
                out.push(FnInfo {
                    name: self.fn_name(n),
                    start: n.start_position().row,
                    end: n.end_position().row,
                    branches: branches(&flow),
                    flow,
                });
            }
            let mut c = n.walk();
            let kids: Vec<Node> = n.named_children(&mut c).collect();
            for k in kids.into_iter().rev() {
                stack.push(k);
            }
        }
        out.sort_by_key(|f| f.start);
    }
}

fn hdr_end(n: Node, body: Option<Node>) -> usize {
    let s = n.start_position().row;
    match body {
        Some(b) => {
            let r = b.start_position().row;
            let first = {
                let mut c = b.walk();
                let r = b.named_children(&mut c).next().map(|x| x.start_position().row);
                r
            };
            let r = match first {
                Some(f) if BLOCK.contains(&b.kind()) && f == r => r.saturating_sub(1),
                _ => r,
            };
            r.max(s)
        }
        None => s,
    }
}

fn branches(f: &Flow) -> usize {
    match f {
        Flow::Seq(v) => v.iter().map(branches).sum(),
        Flow::Stmt(_) | Flow::Jump { .. } => 0,
        Flow::If { then, els, .. } => 1 + branches(then) + els.as_ref().map_or(0, |e| branches(e)),
        Flow::Loop { body, .. } => 1 + branches(body),
        Flow::Switch { arms, .. } => {
            arms.len().saturating_sub(1).max(1) + arms.iter().map(|a| branches(&a.1)).sum::<usize>()
        }
    }
}

pub fn parse(lang: Lang, src: &str) -> Vec<FnInfo> {
    let mut parser = Parser::new();
    if parser.set_language(&language(lang)).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(src, None) else {
        return Vec::new();
    };
    let p = P {
        src: src.as_bytes(),
        lang,
    };
    let mut out = Vec::new();
    p.collect(tree.root_node(), &mut out);
    out
}

pub fn innermost(fns: &[FnInfo], line: usize) -> Option<usize> {
    fns.iter()
        .enumerate()
        .filter(|(_, f)| f.start <= line && line <= f.end)
        .min_by_key(|(_, f)| f.end - f.start)
        .map(|(i, _)| i)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BoxKind {
    Start,
    End,
    Stmt,
    Cond,
    Loop,
    Jump,
    Func,
    File,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    None,
    Func(usize),
    File(PathBuf),
}

#[derive(Clone, Debug)]
pub struct NodeBox {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub kind: BoxKind,
    pub label: String,
    pub sub: String,
    pub lines: (usize, usize),
    pub items: Vec<Item>,
    pub link: Link,
}

impl NodeBox {
    pub fn contains_line(&self, l: usize) -> bool {
        self.lines.0 <= l && l <= self.lines.1
    }

    pub fn hit(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    pub fn text_for(&self, active: Option<usize>) -> &str {
        if let Some(l) = active {
            if self.items.len() > 1 && self.contains_line(l) {
                if let Some(it) = self.items.iter().rev().find(|i| i.line <= l) {
                    return &it.text;
                }
            }
        }
        &self.label
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Seg {
    pub x1: i32,
    pub y1: i32,
    pub x2: i32,
    pub y2: i32,
}

#[derive(Clone, Default, Debug)]
pub struct Graph {
    pub w: i32,
    pub h: i32,
    pub boxes: Vec<NodeBox>,
    pub segs: Vec<Seg>,
    pub arrows: Vec<(i32, i32, char)>,
    pub texts: Vec<(i32, i32, String)>,
}

impl Graph {
    fn absorb(&mut self, o: Graph, dx: i32, dy: i32) {
        for mut b in o.boxes {
            b.x += dx;
            b.y += dy;
            self.boxes.push(b);
        }
        for s in o.segs {
            self.segs.push(Seg {
                x1: s.x1 + dx,
                y1: s.y1 + dy,
                x2: s.x2 + dx,
                y2: s.y2 + dy,
            });
        }
        for (x, y, c) in o.arrows {
            self.arrows.push((x + dx, y + dy, c));
        }
        for (x, y, t) in o.texts {
            self.texts.push((x + dx, y + dy, t));
        }
    }

    fn seg(&mut self, x1: i32, y1: i32, x2: i32, y2: i32) {
        self.segs.push(Seg {
            x1: x1.min(x2),
            y1: y1.min(y2),
            x2: x1.max(x2),
            y2: y1.max(y2),
        });
    }

    pub fn find_line(&self, line: usize) -> Option<usize> {
        self.boxes
            .iter()
            .enumerate()
            .filter(|(_, b)| b.contains_line(line))
            .min_by_key(|(_, b)| (b.lines.1 - b.lines.0, b.kind == BoxKind::Func))
            .map(|(i, _)| i)
    }

    pub fn hit(&self, x: i32, y: i32) -> Option<usize> {
        self.boxes.iter().position(|b| b.hit(x, y))
    }
}

struct Blk {
    g: Graph,
    cx: i32,
    term: bool,
}

impl Blk {
    fn empty() -> Blk {
        Blk {
            g: Graph {
                w: 1,
                ..Graph::default()
            },
            cx: 0,
            term: false,
        }
    }
}

const MINW: i32 = 9;
const MAXW: i32 = 32;

pub fn dispw(s: &str) -> i32 {
    s.chars().map(|c| c.width().unwrap_or(0) as i32).sum()
}

pub fn trunc(s: &str, max: i32) -> String {
    if dispw(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0) as i32;
        if w + cw > max - 1 {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

fn node(kind: BoxKind, label: String, lines: (usize, usize), items: Vec<Item>) -> Blk {
    let w = (dispw(&label) + 4).clamp(MINW, MAXW);
    let b = NodeBox {
        x: 0,
        y: 0,
        w,
        h: 3,
        kind,
        label,
        sub: String::new(),
        lines,
        items,
        link: Link::None,
    };
    Blk {
        g: Graph {
            w,
            h: 3,
            boxes: vec![b],
            ..Graph::default()
        },
        cx: w / 2,
        term: kind == BoxKind::Jump,
    }
}

fn connect(g: &mut Graph, x: i32, from: i32, to: i32) {
    g.seg(x, from, x, to - 1);
    g.arrows.push((x, to - 1, '▼'));
}

fn seq(blks: Vec<Blk>) -> Blk {
    let blks: Vec<Blk> = blks.into_iter().filter(|b| b.g.h > 0).collect();
    if blks.is_empty() {
        return Blk::empty();
    }
    let c = blks.iter().map(|b| b.cx).max().unwrap_or(0);
    let mut g = Graph::default();
    let mut y = 0;
    let mut prev_end: Option<i32> = None;
    let mut term = false;
    let mut w = 0;
    for b in blks {
        if let Some(pe) = prev_end {
            if !term {
                connect(&mut g, c, pe, y);
            }
        }
        let dx = c - b.cx;
        w = w.max(dx + b.g.w);
        let h = b.g.h;
        term |= b.term;
        g.absorb(b.g, dx, y);
        prev_end = Some(y + h - 1);
        y += h + 2;
    }
    g.w = w;
    g.h = y - 2;
    Blk { g, cx: c, term }
}

fn branch(d: Blk, arms: Vec<(String, Blk)>) -> Blk {
    let gap = 3;
    let mut g = Graph::default();
    let mut xs = Vec::new();
    let mut x = 0;
    for (label, b) in &arms {
        let lw = (dispw(label) + 2).min(16);
        let cx = b.cx;
        let aw = b.g.w.max(cx + 1 + lw);
        xs.push((x, x + cx));
        x += aw + gap;
    }
    let first = xs.first().map_or(0, |a| a.1);
    let last = xs.last().map_or(0, |a| a.1);
    let mut dc = (first + last) / 2;
    let mut dx = dc - d.cx;
    let shift = if dx < 0 { -dx } else { 0 };
    dx += shift;
    dc += shift;
    for a in xs.iter_mut() {
        a.0 += shift;
        a.1 += shift;
    }
    let dw = d.g.w;
    g.absorb(d.g, dx, 0);
    let lo = xs.iter().map(|a| a.1).min().unwrap_or(dc).min(dc);
    let hi = xs.iter().map(|a| a.1).max().unwrap_or(dc).max(dc);
    g.seg(dc, 2, dc, 3);
    g.seg(lo, 3, hi, 3);
    let by = 6;
    let hb = arms.iter().map(|a| a.1.g.h).max().unwrap_or(0);
    let mb = by + hb + 1;
    let mut w = dx + dw;
    let mut open = Vec::new();
    for ((label, b), (ax, acx)) in arms.into_iter().zip(xs) {
        let lw = (dispw(&label) + 2).min(16);
        if !label.is_empty() {
            g.texts.push((acx + 1, 4, trunc(&label, lw - 1)));
        }
        w = w.max(ax + b.g.w).max(acx + 1 + lw);
        if b.g.h == 0 {
            g.seg(acx, 3, acx, mb);
            open.push(acx);
            continue;
        }
        connect(&mut g, acx, 3, by);
        let h = b.g.h;
        let t = b.term;
        g.absorb(b.g, ax, by);
        if !t {
            g.seg(acx, by + h - 1, acx, mb);
            open.push(acx);
        }
    }
    if open.is_empty() {
        g.w = w;
        g.h = by + hb;
        return Blk { g, cx: dc, term: true };
    }
    let lo = open.iter().copied().min().unwrap_or(dc).min(dc);
    let hi = open.iter().copied().max().unwrap_or(dc).max(dc);
    g.seg(lo, mb, hi, mb);
    g.w = w;
    g.h = mb + 1;
    Blk { g, cx: dc, term: false }
}

fn loop_blk(h: Blk, body: Blk) -> Blk {
    let ml = 3;
    let c = h.cx.max(body.cx) + ml;
    let hx = c - h.cx;
    let hw = h.g.w;
    let mut g = Graph::default();
    g.absorb(h.g, hx, 0);
    let by = 5;
    let bh = body.g.h;
    let bw = body.g.w;
    let bx = c - body.cx;
    let bterm = body.term;
    let b1 = if bh > 0 {
        connect(&mut g, c, 2, by);
        g.absorb(body.g, bx, by);
        by + bh
    } else {
        3
    };
    if !bterm {
        g.seg(c, b1 - 1, c, b1);
        g.seg(1, b1, c, b1);
        g.seg(1, 1, 1, b1);
        g.seg(1, 1, hx - 2, 1);
        g.arrows.push((hx - 1, 1, '▶'));
    }
    let xr = (hx + hw).max(if bh > 0 { bx + bw } else { 0 }) + 1;
    let b2 = b1 + 1;
    g.seg(hx + hw - 1, 1, xr, 1);
    g.seg(xr, 1, xr, b2);
    g.seg(c, b2, xr, b2);
    g.w = xr + 1;
    g.h = b2 + 1;
    Blk { g, cx: c, term: false }
}

fn lay(f: &Flow) -> Blk {
    match f {
        Flow::Seq(v) => seq(v.iter().map(lay).collect()),
        Flow::Stmt(items) => {
            let first = items.first().map(|i| i.text.clone()).unwrap_or_default();
            let label = if items.len() > 1 {
                format!("{}  +{}", first, items.len() - 1)
            } else {
                first
            };
            let lo = items.first().map_or(0, |i| i.line);
            let hi = items.last().map_or(0, |i| i.end);
            let mut b = node(BoxKind::Stmt, label, (lo, hi), items.clone());
            if items.len() > 1 {
                b.g.boxes[0].label = items[0].text.clone();
                b.g.boxes[0].sub = format!("+{}", items.len() - 1);
            }
            b
        }
        Flow::Jump { text, line, end, .. } => node(BoxKind::Jump, text.clone(), (*line, *end), Vec::new()),
        Flow::If {
            cond,
            line,
            end,
            then,
            els,
        } => {
            let d = node(BoxKind::Cond, cond.clone(), (*line, *end), Vec::new());
            let yes = t!("evet", "yes").to_string();
            let no = t!("hayır", "no").to_string();
            let r = els.as_ref().map_or_else(Blk::empty, |e| lay(e));
            branch(d, vec![(yes, lay(then)), (no, r)])
        }
        Flow::Loop { head, line, end, body } => {
            let h = node(BoxKind::Loop, format!("↻ {}", head), (*line, *end), Vec::new());
            loop_blk(h, lay(body))
        }
        Flow::Switch { head, line, end, arms } => {
            let d = node(BoxKind::Cond, head.clone(), (*line, *end), Vec::new());
            if arms.is_empty() {
                return d;
            }
            branch(d, arms.iter().map(|(l, f)| (l.clone(), lay(f))).collect())
        }
    }
}

pub fn function_graph(f: &FnInfo) -> Graph {
    let start = node(BoxKind::Start, format!("{}()", f.name), (f.start, f.start), Vec::new());
    let body = lay(&f.flow);
    let term = body.term;
    let mut parts = vec![start, body];
    if !term {
        parts.push(node(
            BoxKind::End,
            t!("son", "end").to_string(),
            (f.end, f.end),
            Vec::new(),
        ));
    }
    let b = seq(parts);
    let (bw, bh) = (b.g.w, b.g.h);
    let mut g = Graph::default();
    g.absorb(b.g, 2, 1);
    g.w = bw + 4;
    g.h = bh + 2;
    g
}

pub struct GridItem {
    pub label: String,
    pub sub: String,
    pub lines: (usize, usize),
    pub link: Link,
    pub kind: BoxKind,
}

pub fn grid(items: Vec<GridItem>, view_w: i32) -> Graph {
    let bw = 26;
    let cols = ((view_w - 2) / (bw + 2)).max(1);
    let mut g = Graph::default();
    for (i, it) in items.into_iter().enumerate() {
        let c = i as i32 % cols;
        let r = i as i32 / cols;
        g.boxes.push(NodeBox {
            x: 1 + c * (bw + 2),
            y: 1 + r * 5,
            w: bw,
            h: 4,
            kind: it.kind,
            label: it.label,
            sub: it.sub,
            lines: it.lines,
            items: Vec::new(),
            link: it.link,
        });
    }
    g.w = g.boxes.iter().map(|b| b.x + b.w).max().unwrap_or(0) + 1;
    g.h = g.boxes.iter().map(|b| b.y + b.h).max().unwrap_or(0) + 1;
    g
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Empty,
    Line,
    Arrow,
    Border,
    Text,
    Sub,
    Label,
}

#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub ch: char,
    pub role: Role,
    pub node: u32,
}

pub const NONE: u32 = u32::MAX;

pub struct Canvas {
    pub w: i32,
    pub h: i32,
    pub cells: Vec<Cell>,
}

const U: u8 = 1;
const D: u8 = 2;
const L: u8 = 4;
const R: u8 = 8;

fn bits_char(b: u8) -> char {
    match b {
        x if x == U | D => '│',
        x if x == L | R => '─',
        x if x == D | R => '╭',
        x if x == D | L => '╮',
        x if x == U | R => '╰',
        x if x == U | L => '╯',
        x if x == U | D | R => '├',
        x if x == U | D | L => '┤',
        x if x == L | R | D => '┬',
        x if x == L | R | U => '┴',
        x if x == U | D | L | R => '┼',
        U | D => '│',
        L | R => '─',
        _ => ' ',
    }
}

impl Canvas {
    pub fn get(&self, x: i32, y: i32) -> Option<&Cell> {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return None;
        }
        self.cells.get((y * self.w + x) as usize)
    }

    fn set(&mut self, x: i32, y: i32, ch: char, role: Role, node: u32) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let i = (y * self.w + x) as usize;
        self.cells[i] = Cell { ch, role, node };
    }

    fn text(&mut self, x: i32, y: i32, s: &str, role: Role, node: u32) {
        let mut cx = x;
        for c in s.chars() {
            let w = c.width().unwrap_or(0) as i32;
            if w == 0 {
                continue;
            }
            self.set(cx, y, c, role, node);
            for k in 1..w {
                self.set(cx + k, y, '\0', role, node);
            }
            cx += w;
        }
    }
}

fn border_chars(k: BoxKind) -> [char; 6] {
    match k {
        BoxKind::Start | BoxKind::End | BoxKind::Func | BoxKind::File | BoxKind::Loop => ['╭', '╮', '╰', '╯', '─', '│'],
        BoxKind::Jump => ['┏', '┓', '┗', '┛', '━', '┃'],
        BoxKind::Cond => ['╱', '╲', '╲', '╱', '─', '│'],
        BoxKind::Stmt => ['┌', '┐', '└', '┘', '─', '│'],
    }
}

pub fn rasterize(g: &Graph, active: Option<usize>) -> Canvas {
    let w = g.w.max(1);
    let h = g.h.max(1);
    let mut cv = Canvas {
        w,
        h,
        cells: vec![
            Cell {
                ch: ' ',
                role: Role::Empty,
                node: NONE
            };
            (w * h) as usize
        ],
    };
    let mut bits = vec![0u8; (w * h) as usize];
    let idx = |x: i32, y: i32| -> Option<usize> {
        if x < 0 || y < 0 || x >= w || y >= h {
            None
        } else {
            Some((y * w + x) as usize)
        }
    };
    for s in &g.segs {
        if s.y1 == s.y2 {
            for x in s.x1..=s.x2 {
                if let Some(i) = idx(x, s.y1) {
                    if x > s.x1 {
                        bits[i] |= L;
                    }
                    if x < s.x2 {
                        bits[i] |= R;
                    }
                }
            }
        } else {
            for y in s.y1..=s.y2 {
                if let Some(i) = idx(s.x1, y) {
                    if y > s.y1 {
                        bits[i] |= U;
                    }
                    if y < s.y2 {
                        bits[i] |= D;
                    }
                }
            }
        }
    }
    for y in 0..h {
        for x in 0..w {
            let b = bits[(y * w + x) as usize];
            if b != 0 {
                cv.set(x, y, bits_char(b), Role::Line, NONE);
            }
        }
    }
    for (x, y, c) in &g.arrows {
        cv.set(*x, *y, *c, Role::Arrow, NONE);
    }
    for (x, y, t) in &g.texts {
        cv.text(*x, *y, t, Role::Label, NONE);
    }
    let bit_at = |x: i32, y: i32| idx(x, y).map_or(0, |i| bits[i]);
    for (n, b) in g.boxes.iter().enumerate() {
        let id = n as u32;
        let [tl, tr, bl, br, hz, vt] = border_chars(b.kind);
        let (x0, y0, x1, y1) = (b.x, b.y, b.x + b.w - 1, b.y + b.h - 1);
        for y in y0..=y1 {
            for x in x0..=x1 {
                cv.set(x, y, ' ', Role::Text, id);
            }
        }
        for x in x0 + 1..x1 {
            let top = if bit_at(x, y0 - 1) & D != 0 { '┴' } else { hz };
            let bot = if bit_at(x, y1 + 1) & U != 0 { '┬' } else { hz };
            cv.set(x, y0, top, Role::Border, id);
            cv.set(x, y1, bot, Role::Border, id);
        }
        for y in y0 + 1..y1 {
            let (l, r) = if b.kind == BoxKind::Cond {
                ('⟨', '⟩')
            } else {
                (vt, vt)
            };
            let l = if bit_at(x0 - 1, y) & R != 0 { '┤' } else { l };
            let r = if bit_at(x1 + 1, y) & L != 0 { '├' } else { r };
            cv.set(x0, y, l, Role::Border, id);
            cv.set(x1, y, r, Role::Border, id);
        }
        cv.set(x0, y0, tl, Role::Border, id);
        cv.set(x1, y0, tr, Role::Border, id);
        cv.set(x0, y1, bl, Role::Border, id);
        cv.set(x1, y1, br, Role::Border, id);
        let inner = b.w - 4;
        let act = active.filter(|l| b.contains_line(*l));
        let main = b.text_for(act);
        if b.h >= 4 {
            cv.text(x0 + 2, y0 + 1, &trunc(main, inner), Role::Text, id);
            cv.text(x0 + 2, y0 + 2, &trunc(&b.sub, inner), Role::Sub, id);
        } else if !b.sub.is_empty() && act.is_none() {
            let sw = dispw(&b.sub);
            let t = trunc(main, inner - sw - 1);
            cv.text(x0 + 2, y0 + 1, &t, Role::Text, id);
            cv.text(x1 - 1 - sw, y0 + 1, &b.sub, Role::Sub, id);
        } else {
            cv.text(x0 + 2, y0 + 1, &trunc(main, inner), Role::Text, id);
        }
    }
    cv
}

pub const BRAILLE_BITS: [[u16; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_flow() {
        let src = "fn f(x: i32) -> i32 {\n    let mut t = 0;\n    for i in 0..x {\n        if i % 2 == 0 {\n            t += i;\n        } else {\n            continue;\n        }\n    }\n    match t {\n        0 => return 1,\n        _ => {}\n    }\n    t\n}\n";
        let fns = parse(Lang::Rust, src);
        assert_eq!(fns.len(), 1);
        assert_eq!(fns[0].name, "f");
        assert!(fns[0].branches >= 3);
        let g = function_graph(&fns[0]);
        assert!(g.boxes.iter().any(|b| b.kind == BoxKind::Loop));
        assert!(g.boxes.iter().any(|b| b.kind == BoxKind::Cond));
        let cv = rasterize(&g, Some(4));
        assert_eq!(cv.cells.len() as i32, cv.w * cv.h);
        if std::env::var("FLOW_DUMP").is_ok() {
            for y in 0..cv.h {
                let row: String = (0..cv.w)
                    .map(|x| cv.get(x, y).unwrap().ch)
                    .filter(|c| *c != '\0')
                    .collect();
                println!("{}", row.trim_end());
            }
        }
    }

    #[test]
    fn python_and_c() {
        let py = "class A:\n    def m(self, x):\n        if x:\n            return 1\n        elif x > 2:\n            pass\n        else:\n            raise E()\n        while x:\n            x -= 1\n";
        let f = parse(Lang::Python, py);
        assert_eq!(f[0].name, "A.m");
        let g = function_graph(&f[0]);
        let lp = g.boxes.iter().find(|b| b.kind == BoxKind::Loop).unwrap();
        assert_eq!(lp.lines, (8, 8));
        let c = "int main(void) {\n  int s = 0;\n  switch (s) {\n    case 0: s = 1; break;\n    default: s = 2;\n  }\n  return s;\n}\n";
        let f = parse(Lang::C, c);
        assert_eq!(f[0].name, "main");
        let g = function_graph(&f[0]);
        assert!(g.boxes.iter().any(|b| b.kind == BoxKind::Jump));
        assert!(!g.boxes.iter().any(|b| b.label == "break;"));
    }

    #[test]
    fn go_and_ts() {
        let go = "package m\nfunc (s *Srv) Run(n int) int {\n\tfor i := 0; i < n; i++ {\n\t\tswitch i {\n\t\tcase 1:\n\t\t\treturn i\n\t\t}\n\t}\n\treturn 0\n}\n";
        let f = parse(Lang::Go, go);
        assert_eq!(f[0].name, "Srv.Run");
        let ts =
            "class K { go(a: number) { try { a++ } catch (e) { a-- } } }\nconst h = (x) => { if (x) { return 1 } }\n";
        let f = parse(Lang::Ts, ts);
        let names: Vec<&str> = f.iter().map(|x| x.name.as_str()).collect();
        assert!(names.contains(&"K.go"));
        assert!(names.contains(&"h"));
    }
}
