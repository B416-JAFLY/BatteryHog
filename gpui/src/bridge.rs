use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

pub struct Bridge {
    child: Child,
    requests: Sender<Value>,
    pub events: Receiver<Value>,
    next_id: u64,
}
impl Bridge {
    pub fn start() -> Result<Self, String> {
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let service = std::env::var_os("BATTERY_HOG_SERVICE")
            .map(PathBuf::from)
            .unwrap_or_else(|| executable.with_file_name("batteryhog-service"));
        let mut command = Command::new(service);
        if let Ok(lang) = std::env::var("BATTERY_HOG_LANGUAGE") {
            let lang = crate::locale::language(&[lang]);
            command.args(["-AppleLanguages", &format!("({lang})")]);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| e.to_string())?;
        let mut stdin = child.stdin.take().ok_or("Missing service input")?;
        let stdout = child.stdout.take().ok_or("Missing service output")?;
        let (send, events) = mpsc::channel();
        let (requests, input) = mpsc::channel::<Value>();
        let errors = send.clone();
        thread::spawn(move || {
            for request in input {
                if writeln!(stdin, "{request}")
                    .and_then(|_| stdin.flush())
                    .is_err()
                {
                    let _ = errors.send(json!({"event":"disconnected"}));
                    break;
                }
            }
        });
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if line.len() > 16 * 1024 * 1024 {
                            break;
                        }
                        if let Ok(event) = serde_json::from_str(&line) {
                            if send.send(event).is_err() {
                                return;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = send.send(json!({"event":"disconnected"}));
        });
        Ok(Self {
            child,
            requests,
            events,
            next_id: 0,
        })
    }
    pub fn request(&mut self, method: &str, path: &str, body: Value) -> Result<u64, String> {
        self.next_id += 1;
        self.requests
            .send(json!({"id":self.next_id,"method":method,"path":path,"body":body}))
            .map_err(|e| e.to_string())?;
        Ok(self.next_id)
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
