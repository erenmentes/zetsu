use similar::{capture_diff_slices_deadline, group_diff_ops, Algorithm, DiffOp, DiffTag};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::thread;
use std::time::{Duration, Instant};

pub fn find_root(dir: &Path) -> Option<PathBuf> {
    let mut d = Some(dir);
    while let Some(p) = d {
        if p.join(".git").exists() {
            return Some(p.to_path_buf());
        }
        d = p.parent();
    }
    None
}

pub fn available() -> bool {
    crate::lsp::which("git").is_some()
}

pub fn run(root: &Path, args: &[&str], input: Option<&str>) -> Result<String, String> {
    let mut c = Command::new("git");
    c.arg("-c")
        .arg("core.quotepath=off")
        .arg("-c")
        .arg("color.ui=false")
        .args(args)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::dap::detach(&mut c);
    let mut child = c.spawn().map_err(|e| format!("git: {}", e))?;
    if let (Some(s), Some(mut si)) = (input, child.stdin.take()) {
        let data = s.as_bytes().to_vec();
        thread::spawn(move || {
            let _ = si.write_all(&data);
        });
    }
    let out = child.wait_with_output().map_err(|e| format!("git: {}", e))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        Ok(stdout)
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() { stdout.trim().to_string() } else { err })
    }
}

pub fn spawn<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Receiver<T> {
    let (tx, rx) = channel();
    thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx
}

pub fn rel(root: &Path, p: &Path) -> Option<String> {
    let rk = crate::lsp::path_key(root);
    let pk = crate::lsp::path_key(p);
    let rk = rk.trim_end_matches(['/', '\\']);
    if !pk.starts_with(rk) || pk.len() == rk.len() {
        return None;
    }
    let n = root.components().count();
    let parts: Vec<String> = p
        .components()
        .skip(n)
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub rel: String,
    pub path: PathBuf,
    pub x: char,
    pub y: char,
    pub orig: Option<String>,
}

impl Entry {
    pub fn conflict(&self) -> bool {
        matches!((self.x, self.y), ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D'))
    }

    pub fn untracked(&self) -> bool {
        self.x == '?'
    }

    pub fn staged(&self) -> bool {
        !self.conflict() && !self.untracked() && self.x != ' ' && self.x != '!'
    }

    pub fn unstaged(&self) -> bool {
        !self.conflict() && (self.untracked() || (self.y != ' ' && self.y != '!'))
    }

    pub fn code(&self, staged: bool) -> char {
        if self.conflict() {
            return '!';
        }
        if self.untracked() {
            return 'U';
        }
        let c = if staged { self.x } else { self.y };
        match c {
            'M' | 'T' => 'M',
            'A' => 'A',
            'D' => 'D',
            'R' => 'R',
            'C' => 'C',
            other => other,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub branch: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub entries: Vec<Entry>,
    pub detached: bool,
    pub merging: bool,
}

pub fn status(root: &Path) -> Result<Status, String> {
    let out = run(
        root,
        &["status", "--porcelain=v1", "-z", "-b", "--untracked-files=all"],
        None,
    )?;
    let mut st = Status::default();
    let mut parts = out.split('\0').peekable();
    while let Some(rec) = parts.next() {
        if rec.is_empty() {
            continue;
        }
        if let Some(b) = rec.strip_prefix("## ") {
            parse_branch(b, &mut st);
            continue;
        }
        if rec.len() < 4 {
            continue;
        }
        let mut ch = rec.chars();
        let x = ch.next().unwrap_or(' ');
        let y = ch.next().unwrap_or(' ');
        let path = rec[3..].to_string();
        let orig = if x == 'R' || x == 'C' {
            parts.next().map(|s| s.to_string())
        } else {
            None
        };
        st.entries.push(Entry {
            path: root.join(&path),
            rel: path,
            x,
            y,
            orig,
        });
    }
    st.merging = root.join(".git").join("MERGE_HEAD").exists();
    Ok(st)
}

fn parse_branch(b: &str, st: &mut Status) {
    let (head, rest) = match b.find(" [") {
        Some(i) => (&b[..i], &b[i + 2..]),
        None => (b, ""),
    };
    if let Some(name) = head.strip_prefix("No commits yet on ") {
        st.branch = name.to_string();
    } else if head.starts_with("HEAD (no branch)") {
        st.branch = "HEAD".into();
        st.detached = true;
    } else if let Some((l, u)) = head.split_once("...") {
        st.branch = l.to_string();
        st.upstream = Some(u.to_string());
    } else {
        st.branch = head.to_string();
    }
    for part in rest.trim_end_matches(']').split(", ") {
        if let Some(n) = part.strip_prefix("ahead ") {
            st.ahead = n.parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            st.behind = n.parse().unwrap_or(0);
        }
    }
}

pub fn base_text(root: &Path, rel: &str) -> Option<String> {
    run(root, &["show", &format!(":{}", rel)], None).ok()
}

pub fn split_lines(s: &str) -> Vec<String> {
    let mut v: Vec<String> = s
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect();
    if s.ends_with('\n') {
        v.pop();
    }
    v
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hunk {
    pub old_start: usize,
    pub old_len: usize,
    pub new_start: usize,
    pub new_len: usize,
    pub old_lines: Vec<String>,
    pub new_lines: Vec<String>,
}

impl Hunk {
    pub fn covers(&self, line: usize) -> bool {
        if self.new_len == 0 {
            line == self.new_start.saturating_sub(1) || line == self.new_start
        } else {
            line >= self.new_start && line < self.new_start + self.new_len
        }
    }
}

fn ops(old: &[String], new: &[String]) -> Vec<DiffOp> {
    capture_diff_slices_deadline(
        Algorithm::Myers,
        old,
        new,
        Some(Instant::now() + Duration::from_millis(200)),
    )
}

pub fn hunks(old: &[String], new: &[String]) -> Vec<Hunk> {
    let mut out: Vec<Hunk> = Vec::new();
    let mut cur: Option<Hunk> = None;
    for op in ops(old, new) {
        let (tag, o, n) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            if let Some(h) = cur.take() {
                out.push(h);
            }
            continue;
        }
        let h = cur.get_or_insert_with(|| Hunk {
            old_start: o.start,
            old_len: 0,
            new_start: n.start,
            new_len: 0,
            old_lines: Vec::new(),
            new_lines: Vec::new(),
        });
        h.old_len += o.len();
        h.new_len += n.len();
        h.old_lines.extend(old[o].iter().cloned());
        h.new_lines.extend(new[n].iter().cloned());
    }
    if let Some(h) = cur {
        out.push(h);
    }
    out
}

pub fn unified(old: &[String], new: &[String], a: &str, b: &str) -> String {
    let groups = group_diff_ops(ops(old, new), 3);
    if groups.is_empty() {
        return String::new();
    }
    let mut s = format!("--- a/{}\n+++ b/{}\n", a, b);
    for g in groups {
        let (Some(first), Some(last)) = (g.first(), g.last()) else {
            continue;
        };
        let os = first.old_range().start;
        let ns = first.new_range().start;
        let ol = last.old_range().end - os;
        let nl = last.new_range().end - ns;
        s.push_str(&format!("@@ -{} +{} @@\n", range_str(os, ol), range_str(ns, nl)));
        for op in &g {
            let (tag, o, n) = op.as_tag_tuple();
            match tag {
                DiffTag::Equal => {
                    for l in &old[o] {
                        s.push_str(&format!(" {}\n", l));
                    }
                }
                _ => {
                    for l in &old[o] {
                        s.push_str(&format!("-{}\n", l));
                    }
                    for l in &new[n] {
                        s.push_str(&format!("+{}\n", l));
                    }
                }
            }
        }
    }
    s
}

fn range_str(start: usize, len: usize) -> String {
    match len {
        0 => format!("{},0", start),
        1 => format!("{}", start + 1),
        _ => format!("{},{}", start + 1, len),
    }
}

pub fn hunk_patch(rel: &str, h: &Hunk, old_ends_newline: bool, old_total: usize) -> String {
    let mut s = format!("--- a/{0}\n+++ b/{0}\n", rel);
    let os = if h.old_len == 0 { h.old_start } else { h.old_start + 1 };
    let ns = if h.new_len == 0 { h.old_start } else { h.old_start + 1 };
    s.push_str(&format!("@@ -{},{} +{},{} @@\n", os, h.old_len, ns, h.new_len));
    for (i, l) in h.old_lines.iter().enumerate() {
        s.push_str(&format!("-{}\n", l));
        if !old_ends_newline && h.old_start + i + 1 == old_total {
            s.push_str("\\ No newline at end of file\n");
        }
    }
    for l in &h.new_lines {
        s.push_str(&format!("+{}\n", l));
    }
    s
}

#[derive(Clone, Debug)]
pub struct Blame {
    pub author: String,
    pub time: i64,
    pub summary: String,
    pub hash: String,
    pub uncommitted: bool,
}

pub fn blame(root: &Path, rel: &str, line: usize, contents: &str) -> Result<Blame, String> {
    let l = format!("{},+1", line + 1);
    let out = run(
        root,
        &["blame", "--porcelain", "-L", &l, "--contents", "-", "--", rel],
        Some(contents),
    )?;
    let mut b = Blame {
        author: String::new(),
        time: 0,
        summary: String::new(),
        hash: String::new(),
        uncommitted: false,
    };
    for (i, line) in out.lines().enumerate() {
        if i == 0 {
            b.hash = line.split_whitespace().next().unwrap_or("").to_string();
            b.uncommitted = b.hash.chars().all(|c| c == '0');
        } else if let Some(a) = line.strip_prefix("author ") {
            b.author = a.to_string();
        } else if let Some(t) = line.strip_prefix("author-time ") {
            b.time = t.parse().unwrap_or(0);
        } else if let Some(s) = line.strip_prefix("summary ") {
            b.summary = s.to_string();
        }
    }
    Ok(b)
}

pub fn ago(ts: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let d = (now - ts).max(0);
    let (n, tr, en) = if d < 60 {
        return t!("şimdi", "just now").to_string();
    } else if d < 3600 {
        (d / 60, "dk önce", "min ago")
    } else if d < 86_400 {
        (d / 3600, "sa önce", "h ago")
    } else if d < 86_400 * 30 {
        (d / 86_400, "gün önce", "d ago")
    } else if d < 86_400 * 365 {
        (d / (86_400 * 30), "ay önce", "mo ago")
    } else {
        (d / (86_400 * 365), "yıl önce", "y ago")
    };
    if crate::i18n::is_en() {
        format!("{}{}", n, en)
    } else {
        format!("{} {}", n, tr)
    }
}

#[derive(Clone, Debug)]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub author: String,
    pub time: i64,
    pub subject: String,
}

pub fn log(root: &Path, n: usize) -> Result<Vec<Commit>, String> {
    let out = run(
        root,
        &["log", &format!("-n{}", n), "--format=%H%x1f%h%x1f%an%x1f%at%x1f%s"],
        None,
    )?;
    Ok(out
        .lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.split('\x1f').collect();
            (p.len() == 5).then(|| Commit {
                hash: p[0].into(),
                short: p[1].into(),
                author: p[2].into(),
                time: p[3].parse().unwrap_or(0),
                subject: p[4].into(),
            })
        })
        .collect())
}

pub fn branches(root: &Path) -> Result<Vec<String>, String> {
    let out = run(root, &["branch", "--format=%(refname:short)"], None)?;
    Ok(out
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

pub fn remotes(root: &Path) -> Vec<String> {
    run(root, &["remote"], None)
        .map(|o| {
            o.lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Conflict {
    pub start: usize,
    pub base: Option<usize>,
    pub mid: usize,
    pub end: usize,
}

pub fn conflicts(lines: &[Vec<char>]) -> Vec<Conflict> {
    let starts = |l: &[char], p: &str| -> bool {
        let n = p.chars().count();
        l.len() >= n && l.iter().take(n).copied().eq(p.chars()) && (l.len() == n || l[n] == ' ')
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if starts(&lines[i], "<<<<<<<") {
            let start = i;
            let mut base = None;
            let mut mid = None;
            let mut j = i + 1;
            while j < lines.len() {
                let l = &lines[j];
                if starts(l, "<<<<<<<") {
                    break;
                }
                if mid.is_none() && base.is_none() && starts(l, "|||||||") {
                    base = Some(j);
                } else if mid.is_none() && l.len() == 7 && l.iter().all(|c| *c == '=') {
                    mid = Some(j);
                } else if mid.is_some() && starts(l, ">>>>>>>") {
                    out.push(Conflict {
                        start,
                        base,
                        mid: mid.unwrap_or(start),
                        end: j,
                    });
                    break;
                }
                j += 1;
            }
            i = j.max(i + 1);
            continue;
        }
        i += 1;
    }
    out
}

pub fn resolve(lines: &[Vec<char>], c: &Conflict, ours: bool, theirs: bool) -> Vec<Vec<char>> {
    let ours_end = c.base.unwrap_or(c.mid);
    let mut v = Vec::new();
    if ours {
        v.extend(lines[c.start + 1..ours_end].iter().cloned());
    }
    if theirs {
        v.extend(lines[c.mid + 1..c.end].iter().cloned());
    }
    v
}

pub fn diff_targets(text: &str) -> Vec<Option<(String, usize)>> {
    let mut out = Vec::new();
    let mut file: Option<String> = None;
    let mut new_line = 0usize;
    for l in text.lines() {
        if let Some(f) = l.strip_prefix("+++ ") {
            let f = f.trim();
            file = if f == "/dev/null" {
                None
            } else {
                Some(f.strip_prefix("b/").unwrap_or(f).to_string())
            };
            out.push(None);
            continue;
        }
        if l.starts_with("--- ") || l.starts_with("diff ") || l.starts_with("index ") {
            out.push(None);
            continue;
        }
        if let Some(rest) = l.strip_prefix("@@ ") {
            let plus = rest.split_whitespace().find(|p| p.starts_with('+')).unwrap_or("+1");
            let n: usize = plus[1..].split(',').next().and_then(|x| x.parse().ok()).unwrap_or(1);
            new_line = n.saturating_sub(1);
            out.push(file.clone().map(|f| (f, new_line)));
            continue;
        }
        match (l.chars().next(), &file) {
            (Some('+'), Some(f)) | (Some(' '), Some(f)) => {
                out.push(Some((f.clone(), new_line)));
                new_line += 1;
            }
            (Some('-'), Some(f)) => out.push(Some((f.clone(), new_line))),
            _ => out.push(None),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn hunks_and_patch() {
        let old = v(&["a", "b", "c", "d"]);
        let new = v(&["a", "B", "c", "d", "e"]);
        let h = hunks(&old, &new);
        assert_eq!(h.len(), 2);
        assert_eq!(
            (h[0].old_start, h[0].old_len, h[0].new_start, h[0].new_len),
            (1, 1, 1, 1)
        );
        assert_eq!((h[1].old_len, h[1].new_start, h[1].new_len), (0, 4, 1));
        let p = hunk_patch("f.txt", &h[0], true, 4);
        assert!(p.contains("@@ -2,1 +2,1 @@\n-b\n+B\n"));
        let u = unified(&old, &new, "f.txt", "f.txt");
        assert!(u.contains("-b\n+B\n"));
    }

    #[test]
    fn status_branch_line() {
        let mut st = Status::default();
        parse_branch("main...origin/main [ahead 2, behind 1]", &mut st);
        assert_eq!((st.branch.as_str(), st.ahead, st.behind), ("main", 2, 1));
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        let mut st = Status::default();
        parse_branch("No commits yet on main", &mut st);
        assert_eq!(st.branch, "main");
    }

    #[test]
    fn conflict_markers() {
        let lines: Vec<Vec<char>> = ["x", "<<<<<<< HEAD", "ours", "=======", "theirs", ">>>>>>> b", "y"]
            .iter()
            .map(|s| s.chars().collect())
            .collect();
        let c = conflicts(&lines);
        assert_eq!(
            c,
            vec![Conflict {
                start: 1,
                base: None,
                mid: 3,
                end: 5
            }]
        );
        let both = resolve(&lines, &c[0], true, true);
        assert_eq!(both.len(), 2);
        let t = diff_targets("--- a/f\n+++ b/f\n@@ -1,2 +1,3 @@\n a\n+b\n c\n");
        assert_eq!(t[3], Some(("f".into(), 0)));
        assert_eq!(t[4], Some(("f".into(), 1)));
    }
}
