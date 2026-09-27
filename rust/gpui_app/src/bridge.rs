//! JSON-lines link for commands still served by the Python workflow process.
use crate::native;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

pub struct Bridge {
    input: Sender<Value>,
    output: Receiver<Value>,
    native_output: Sender<Value>,
    next_id: u64,
}

impl Bridge {
    pub fn start() -> Result<Self, String> {
        let python = std::env::var("MUZIK_PYTHON").unwrap_or_else(|_| "python3".into());
        let mut child = Command::new(python)
            .args(["-m", "muzik.native_gui"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|err| format!("Cannot start the Python service: {err}"))?;
        let mut stdin = child.stdin.take().ok_or("Cannot open Python input")?;
        let stdout = child.stdout.take().ok_or("Cannot open Python output")?;
        let (input, commands) = mpsc::channel::<Value>();
        let (events, output) = mpsc::channel::<Value>();
        let native_output = events.clone();
        thread::spawn(move || {
            while let Ok(command) = commands.recv() {
                if writeln!(stdin, "{command}").is_err() || stdin.flush().is_err() {
                    break;
                }
            }
        });
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => match serde_json::from_str::<Value>(&line) {
                        Ok(message) => {
                            if events.send(message).is_err() {
                                break;
                            }
                        }
                        Err(err) => {
                            let _ = events.send(json!({"type":"transport.error","message":format!("Invalid Python response: {err}")}));
                        }
                    },
                    Err(err) => {
                        let _ = events.send(json!({"type":"transport.error","message":format!("Python output failed: {err}")}));
                        break;
                    }
                }
            }
            let _ = events.send(json!({"type":"transport.closed"}));
            let _ = child.wait();
        });
        Ok(Self {
            input,
            output,
            native_output,
            next_id: 1,
        })
    }

    pub fn send(&mut self, command: &str, params: Value) -> Result<String, String> {
        let id = self.next_id.to_string();
        self.next_id += 1;
        if command == "library.scan" {
            let sender = self.native_output.clone();
            let response_id = id.clone();
            thread::spawn(move || {
                let response = match native::library_scan(&params) {
                    Ok(result) => {
                        json!({"id": response_id, "type":"response", "ok":true, "result":result})
                    }
                    Err(message) => {
                        json!({"id": response_id, "type":"response", "ok":false, "error":{"code":"operation_failed", "message":message}})
                    }
                };
                let _ = sender.send(response);
            });
            return Ok(id);
        }
        self.input
            .send(json!({"id":id,"command":command,"params":params}))
            .map_err(|_| "Python service is not available".to_string())?;
        Ok(id)
    }

    pub fn drain(&self) -> Vec<Value> {
        self.output.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Bridge;
    use serde_json::{json, Value};
    use std::fs;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn library_scan_uses_rust_without_sending_a_python_request(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join("Track [dQw4w9WgXcQ].mp3"), b"audio")?;
        let (input, python_requests) = mpsc::channel::<Value>();
        let (native_output, output) = mpsc::channel::<Value>();
        let mut bridge = Bridge {
            input,
            output,
            native_output,
            next_id: 1,
        };
        let id = bridge.send("library.scan", json!({"output": dir.path()}))?;
        let response = bridge.output.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(response["id"], id);
        assert_eq!(response["result"]["total_size"], "5.0 B");
        assert!(python_requests.try_recv().is_err());
        Ok(())
    }
}
