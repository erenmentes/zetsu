use serde_json::{json, Value};
use std::io::{self, Read, Write};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const ASSET_KEY: &str = "zetsu";

trait Pipe: Read + Write + Send {}
impl<T: Read + Write + Send> Pipe for T {}

enum Msg {
    Update(String, String),
    SetId(String),
    Enable(bool),
}

pub struct Presence {
    tx: Sender<Msg>,
    pub status: Arc<Mutex<String>>,
    last: Option<(String, String)>,
    pub enabled: bool,
    pub client_id: String,
}

fn connect() -> Option<Box<dyn Pipe>> {
    #[cfg(windows)]
    {
        for i in 0..10 {
            let name = format!(r"\\.\pipe\discord-ipc-{}", i);
            if let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).open(&name) {
                return Some(Box::new(f));
            }
        }
        None
    }
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixStream;
        let mut dirs: Vec<String> = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
            .iter()
            .filter_map(|k| std::env::var(k).ok())
            .collect();
        dirs.push("/tmp".into());
        for d in dirs {
            for sub in [
                "",
                "app/com.discordapp.Discord/",
                "snap.discord/",
                ".flatpak/dev.vencord.Vesktop/xdg-run/",
            ] {
                for i in 0..10 {
                    let p = format!("{}/{}discord-ipc-{}", d.trim_end_matches('/'), sub, i);
                    if let Ok(s) = UnixStream::connect(&p) {
                        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                        return Some(Box::new(s));
                    }
                }
            }
        }
        None
    }
}

fn send(p: &mut dyn Pipe, op: u32, v: &Value) -> io::Result<()> {
    let body = v.to_string();
    let mut buf = Vec::with_capacity(8 + body.len());
    buf.extend_from_slice(&op.to_le_bytes());
    buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
    buf.extend_from_slice(body.as_bytes());
    p.write_all(&buf)?;
    p.flush()
}

fn recv(p: &mut dyn Pipe) -> io::Result<(u32, Value)> {
    let mut h = [0u8; 8];
    p.read_exact(&mut h)?;
    let op = u32::from_le_bytes([h[0], h[1], h[2], h[3]]);
    let len = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as usize;
    let mut body = vec![0u8; len];
    p.read_exact(&mut body)?;
    Ok((op, serde_json::from_slice(&body).unwrap_or(Value::Null)))
}

fn handshake(client_id: &str) -> Result<Box<dyn Pipe>, String> {
    let mut p = connect().ok_or_else(|| t!("Discord çalışmıyor", "Discord is not running").to_string())?;
    send(&mut *p, 0, &json!({ "v": 1, "client_id": client_id })).map_err(|e| e.to_string())?;
    match recv(&mut *p) {
        Ok((1, v)) if v["evt"] == "READY" => Ok(p),
        Ok((_, v)) => Err(v["message"]
            .as_str()
            .unwrap_or(t!("bağlantı reddedildi", "connection refused"))
            .to_string()),
        Err(e) => Err(e.to_string()),
    }
}

fn clip(s: &str) -> String {
    let mut t: String = s.chars().take(120).collect();
    while t.chars().count() < 2 {
        t.push(' ');
    }
    t
}

fn activity_cmd(nonce: u64, act: Option<(&str, &str, u64)>) -> Value {
    let activity = match act {
        Some((details, state, start)) => json!({
            "details": clip(details),
            "state": clip(state),
            "timestamps": { "start": start },
            "assets": { "large_image": ASSET_KEY, "large_text": "Zetsu IDE" },
        }),
        None => Value::Null,
    };
    json!({
        "cmd": "SET_ACTIVITY",
        "args": { "pid": std::process::id(), "activity": activity },
        "nonce": format!("zetsu-{}", nonce),
    })
}

fn worker(rx: std::sync::mpsc::Receiver<Msg>, mut client_id: String, mut enabled: bool, status: Arc<Mutex<String>>) {
    let start = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut conn: Option<Box<dyn Pipe>> = None;
    let mut want: Option<(String, String)> = None;
    let mut sent: Option<(String, String)> = None;
    let mut last_try: Option<Instant> = None;
    let mut last_send = Instant::now() - Duration::from_secs(60);
    let mut nonce = 0u64;
    let set_status = |s: &str| {
        if let Ok(mut g) = status.lock() {
            *g = s.to_string();
        }
    };
    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(Msg::Update(d, s)) => want = Some((d, s)),
            Ok(Msg::SetId(id)) => {
                client_id = id;
                conn = None;
                sent = None;
                last_try = None;
            }
            Ok(Msg::Enable(e)) => {
                enabled = e;
                sent = None;
                last_try = None;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if !enabled || client_id.is_empty() {
            if let Some(mut p) = conn.take() {
                nonce += 1;
                let _ = send(&mut *p, 1, &activity_cmd(nonce, None));
            }
            set_status(if client_id.is_empty() {
                t!("uygulama kimliği ayarlanmadı", "application ID not set")
            } else {
                t!("kapalı", "off")
            });
            continue;
        }
        if conn.is_none() {
            if last_try.is_some_and(|t| t.elapsed() < Duration::from_secs(15)) {
                continue;
            }
            last_try = Some(Instant::now());
            match handshake(&client_id) {
                Ok(p) => {
                    conn = Some(p);
                    sent = None;
                    set_status(t!("bağlı", "connected"));
                }
                Err(e) => {
                    set_status(&e);
                    continue;
                }
            }
        }
        if want != sent && last_send.elapsed() >= Duration::from_secs(4) {
            if let (Some(p), Some((d, s))) = (conn.as_mut(), want.as_ref()) {
                nonce += 1;
                let ok = send(&mut **p, 1, &activity_cmd(nonce, Some((d, s, start)))).and_then(|_| recv(&mut **p));
                last_send = Instant::now();
                match ok {
                    Ok((_, v)) if v["evt"] == "ERROR" => {
                        set_status(v["data"]["message"].as_str().unwrap_or(t!("hata", "error")));
                        sent = want.clone();
                    }
                    Ok(_) => {
                        sent = want.clone();
                        set_status(t!("bağlı", "connected"));
                    }
                    Err(_) => {
                        conn = None;
                        set_status(t!("bağlantı koptu, yeniden denenecek", "disconnected, will retry"));
                    }
                }
            }
        }
    }
    if let Some(mut p) = conn.take() {
        let _ = send(&mut *p, 1, &activity_cmd(nonce + 1, None));
        let _ = recv(&mut *p);
    }
}

impl Presence {
    pub fn start(client_id: String, enabled: bool) -> Self {
        let (tx, rx) = channel();
        let status = Arc::new(Mutex::new(String::from(t!("başlıyor", "starting"))));
        let st = status.clone();
        let id = client_id.clone();
        thread::spawn(move || worker(rx, id, enabled, st));
        Presence {
            tx,
            status,
            last: None,
            enabled,
            client_id,
        }
    }

    pub fn update(&mut self, details: String, state: String) {
        let v = (details, state);
        if self.last.as_ref() != Some(&v) {
            self.last = Some(v.clone());
            let _ = self.tx.send(Msg::Update(v.0, v.1));
        }
    }

    pub fn set_id(&mut self, id: String) {
        self.client_id = id.clone();
        let _ = self.tx.send(Msg::SetId(id));
    }

    pub fn enable(&mut self, on: bool) {
        self.enabled = on;
        let _ = self.tx.send(Msg::Enable(on));
    }

    pub fn status(&self) -> String {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }
}
