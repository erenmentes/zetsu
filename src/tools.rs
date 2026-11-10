use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Tool {
    CodeLldb,
    Debugpy,
    Delve,
}

impl Tool {
    pub const ALL: [Tool; 3] = [Tool::CodeLldb, Tool::Debugpy, Tool::Delve];

    pub fn name(self) -> &'static str {
        match self {
            Tool::CodeLldb => "CodeLLDB",
            Tool::Debugpy => "debugpy",
            Tool::Delve => "Delve",
        }
    }

    pub fn parse(s: &str) -> Option<Tool> {
        match s.to_lowercase().as_str() {
            "codelldb" | "lldb" | "rust" | "c" | "cpp" => Some(Tool::CodeLldb),
            "debugpy" | "python" => Some(Tool::Debugpy),
            "delve" | "dlv" | "go" => Some(Tool::Delve),
            _ => None,
        }
    }

    fn dir(self) -> PathBuf {
        tools_dir().join(match self {
            Tool::CodeLldb => "codelldb",
            Tool::Debugpy => "debugpy",
            Tool::Delve => "delve",
        })
    }
}

pub enum Progress {
    Status(String),
    Bytes(u64, Option<u64>),
    Done(Result<(), String>),
}

pub fn tools_dir() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
    };
    base.unwrap_or_else(std::env::temp_dir).join("zetsu").join("tools")
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{}.exe", name)
    } else {
        name.to_string()
    }
}

pub fn codelldb_path() -> Option<PathBuf> {
    let p = Tool::CodeLldb
        .dir()
        .join("extension")
        .join("adapter")
        .join(exe("codelldb"));
    p.is_file().then_some(p)
}

pub fn debugpy_dir() -> Option<PathBuf> {
    let d = Tool::Debugpy.dir();
    d.join("debugpy").join("__init__.py").is_file().then_some(d)
}

pub fn dlv_path() -> Option<PathBuf> {
    let p = Tool::Delve.dir().join(exe("dlv"));
    p.is_file().then_some(p)
}

pub fn installed(t: Tool) -> bool {
    match t {
        Tool::CodeLldb => codelldb_path().is_some(),
        Tool::Debugpy => debugpy_dir().is_some(),
        Tool::Delve => dlv_path().is_some(),
    }
}

fn codelldb_platform() -> Option<&'static str> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    Some(match (os, arch) {
        ("windows", "x86_64") => "win32-x64",
        ("windows", "aarch64") => "win32-arm64",
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-arm64",
        ("linux", "arm") => "linux-armhf",
        ("macos", "x86_64") => "darwin-x64",
        ("macos", "aarch64") => "darwin-arm64",
        _ => return None,
    })
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .try_proxy_from_env(true)
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(60))
        .user_agent(concat!("zetsu/", env!("CARGO_PKG_VERSION")))
        .build()
}

fn download(url: &str, dest: &Path, tx: &Sender<Progress>) -> Result<(), String> {
    let resp = agent().get(url).call().map_err(|e| e.to_string())?;
    let total = resp.header("Content-Length").and_then(|v| v.parse::<u64>().ok());
    let mut r = resp.into_reader();
    let mut f = fs::File::create(dest).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 1 << 16];
    let mut done: u64 = 0;
    let mut last = 0u64;
    loop {
        let n = r.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if done - last >= 1 << 20 {
            last = done;
            let _ = tx.send(Progress::Bytes(done, total));
        }
    }
    let _ = tx.send(Progress::Bytes(done, total));
    Ok(())
}

fn extract(archive: &Path, dest: &Path, keep: &dyn Fn(&str) -> bool) -> Result<usize, String> {
    let f = fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
    let mut count = 0;
    for i in 0..z.len() {
        let mut entry = z.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().replace('\\', "/");
        if !keep(&name) || name.ends_with('/') {
            continue;
        }
        let Some(rel) = entry.enclosed_name() else { continue };
        let out = dest.join(rel);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut w = fs::File::create(&out).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut w).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(mode) = entry.unix_mode() {
                let _ = fs::set_permissions(&out, fs::Permissions::from_mode(mode & 0o777));
            }
        }
        count += 1;
    }
    Ok(count)
}

fn finish(staging: &Path, target: &Path) -> Result<(), String> {
    if target.exists() {
        fs::remove_dir_all(target).map_err(|e| e.to_string())?;
    }
    fs::rename(staging, target).map_err(|e| e.to_string())
}

fn install_codelldb(tx: &Sender<Progress>) -> Result<(), String> {
    let plat = codelldb_platform().ok_or("CodeLLDB is not available for this platform")?;
    let root = tools_dir();
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let url = format!(
        "https://github.com/vadimcn/codelldb/releases/latest/download/codelldb-{}.vsix",
        plat
    );
    let archive = root.join("codelldb.vsix.part");
    let _ = tx.send(Progress::Status(tf!(
        "CodeLLDB ({}) indiriliyor…",
        "Downloading CodeLLDB ({})…",
        plat
    )));
    download(&url, &archive, tx)?;
    let _ = tx.send(Progress::Status(t!("Açılıyor…", "Extracting…").into()));
    let staging = root.join("codelldb.part");
    let _ = fs::remove_dir_all(&staging);
    let keep = |n: &str| {
        n.starts_with("extension/adapter/")
            || n.starts_with("extension/lldb/")
            || n.starts_with("extension/lang_support/")
    };
    extract(&archive, &staging, &keep)?;
    let _ = fs::remove_file(&archive);
    if !staging
        .join("extension")
        .join("adapter")
        .join(exe("codelldb"))
        .is_file()
    {
        return Err("CodeLLDB package did not contain the adapter".into());
    }
    finish(&staging, &Tool::CodeLldb.dir())
}

fn install_debugpy(tx: &Sender<Progress>) -> Result<(), String> {
    let root = tools_dir();
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let _ = tx.send(Progress::Status(t!("PyPI sorgulanıyor…", "Querying PyPI…").into()));
    let meta: Value = serde_json::from_str(
        &agent()
            .get("https://pypi.org/pypi/debugpy/json")
            .call()
            .map_err(|e| e.to_string())?
            .into_string()
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let wheel = meta["urls"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|u| u["filename"].as_str().is_some_and(|f| f.ends_with("-none-any.whl")))
        .and_then(|u| u["url"].as_str().map(|s| s.to_string()))
        .ok_or("no universal debugpy wheel found on PyPI")?;
    let version = meta["info"]["version"].as_str().unwrap_or("").to_string();
    let _ = tx.send(Progress::Status(tf!(
        "debugpy {} indiriliyor…",
        "Downloading debugpy {}…",
        version
    )));
    let archive = root.join("debugpy.whl.part");
    download(&wheel, &archive, tx)?;
    let _ = tx.send(Progress::Status(t!("Açılıyor…", "Extracting…").into()));
    let staging = root.join("debugpy.part");
    let _ = fs::remove_dir_all(&staging);
    extract(&archive, &staging, &|_| true)?;
    let _ = fs::remove_file(&archive);
    finish(&staging, &Tool::Debugpy.dir())
}

fn install_delve(tx: &Sender<Progress>) -> Result<(), String> {
    let go = crate::lsp::which("go").ok_or("Go is not installed (https://go.dev/dl)")?;
    let dir = Tool::Delve.dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let _ = tx.send(Progress::Status(
        "go install github.com/go-delve/delve/cmd/dlv@latest".into(),
    ));
    let out = Command::new(go)
        .args(["install", "github.com/go-delve/delve/cmd/dlv@latest"])
        .env("GOBIN", &dir)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err
            .lines()
            .rev()
            .take(6)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n"));
    }
    if dlv_path().is_none() {
        return Err("dlv was not produced by go install".into());
    }
    Ok(())
}

pub fn install(t: Tool) -> Receiver<Progress> {
    let (tx, rx) = channel();
    thread::spawn(move || {
        let r = match t {
            Tool::CodeLldb => install_codelldb(&tx),
            Tool::Debugpy => install_debugpy(&tx),
            Tool::Delve => install_delve(&tx),
        };
        let _ = tx.send(Progress::Done(r));
    });
    rx
}

pub fn uninstall(t: Tool) -> std::io::Result<()> {
    let d = t.dir();
    if d.exists() {
        fs::remove_dir_all(d)?;
    }
    Ok(())
}
