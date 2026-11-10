use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const IGNORED: &[&str] = &[".git", "target", "node_modules", "__pycache__", ".DS_Store"];

pub struct Node {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
}

pub struct Tree {
    pub root: PathBuf,
    pub nodes: Vec<Node>,
    pub sel: usize,
    pub scroll: usize,
    pub visible: bool,
    pub width: u16,
}

fn read_dir(p: &Path, depth: usize) -> Vec<Node> {
    let Ok(rd) = fs::read_dir(p) else { return Vec::new() };
    let mut v: Vec<Node> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if IGNORED.contains(&name.as_str()) {
                return None;
            }
            let path = e.path();
            let is_dir = path.is_dir();
            Some(Node {
                path,
                name,
                depth,
                is_dir,
                expanded: false,
            })
        })
        .collect();
    v.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    v
}

fn build(p: &Path, depth: usize, expanded: &HashSet<PathBuf>, out: &mut Vec<Node>) {
    for mut n in read_dir(p, depth) {
        if n.is_dir && expanded.contains(&n.path) {
            n.expanded = true;
            let path = n.path.clone();
            out.push(n);
            build(&path, depth + 1, expanded, out);
        } else {
            out.push(n);
        }
    }
}

impl Tree {
    pub fn new(root: PathBuf) -> Self {
        let nodes = read_dir(&root, 0);
        Tree {
            root,
            nodes,
            sel: 0,
            scroll: 0,
            visible: true,
            width: 28,
        }
    }

    pub fn root_name(&self) -> String {
        self.root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.display().to_string())
    }

    pub fn set_root(&mut self, root: PathBuf) {
        self.nodes = read_dir(&root, 0);
        self.root = root;
        self.sel = 0;
        self.scroll = 0;
    }

    pub fn toggle(&mut self, i: usize) {
        if i >= self.nodes.len() || !self.nodes[i].is_dir {
            return;
        }
        if self.nodes[i].expanded {
            let d = self.nodes[i].depth;
            let mut j = i + 1;
            while j < self.nodes.len() && self.nodes[j].depth > d {
                j += 1;
            }
            self.nodes.drain(i + 1..j);
            self.nodes[i].expanded = false;
            if self.sel > i && self.sel < j {
                self.sel = i;
            }
        } else {
            let kids = read_dir(&self.nodes[i].path, self.nodes[i].depth + 1);
            self.nodes.splice(i + 1..i + 1, kids);
            self.nodes[i].expanded = true;
        }
    }

    pub fn parent_of(&self, i: usize) -> Option<usize> {
        let d = self.nodes.get(i)?.depth;
        if d == 0 {
            return None;
        }
        (0..i).rev().find(|&j| self.nodes[j].depth == d - 1)
    }

    pub fn refresh(&mut self) {
        let expanded: HashSet<PathBuf> = self
            .nodes
            .iter()
            .filter(|n| n.expanded)
            .map(|n| n.path.clone())
            .collect();
        let sel_path = self.nodes.get(self.sel).map(|n| n.path.clone());
        let mut out = Vec::new();
        build(&self.root, 0, &expanded, &mut out);
        self.nodes = out;
        if let Some(p) = sel_path {
            self.sel = self.nodes.iter().position(|n| n.path == p).unwrap_or(0);
        }
        self.sel = self.sel.min(self.nodes.len().saturating_sub(1));
    }
}
