//! JSON-lines link for commands still served by the Python workflow process.
use crate::{native, thumbnails};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

pub struct Bridge {
    input: Option<Sender<Value>>,
    output: Receiver<Value>,
    native_output: Sender<Value>,
    thumbnail_pending: Arc<Mutex<HashSet<String>>>,
    next_id: u64,
}

impl Bridge {
    pub fn start() -> Result<Self, String> {
        let (events, output) = mpsc::channel::<Value>();
        Ok(Self {
            input: None,
            output,
            native_output: events,
            thumbnail_pending: Arc::new(Mutex::new(HashSet::new())),
            next_id: 1,
        })
    }

    fn start_python(&mut self) -> Result<(), String> {
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
        let events = self.native_output.clone();
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
        self.input = Some(input);
        Ok(())
    }

    pub fn send(&mut self, command: &str, params: Value) -> Result<String, String> {
        let id = self.next_id.to_string();
        self.next_id += 1;
        if command == "thumbnails.cache" {
            let ids = match thumbnails::validate_ids(&params) {
                Ok(ids) => ids,
                Err(message) => {
                    self.native_output
                        .send(json!({"id":id,"type":"response","ok":false,"error":{"code":"invalid_request","message":message}}))
                        .map_err(|_| "Rust backend is not available".to_owned())?;
                    return Ok(id);
                }
            };
            let mut pending = self
                .thumbnail_pending
                .lock()
                .map_err(|_| "thumbnail queue is not available")?;
            let fresh = ids
                .into_iter()
                .filter(|id| pending.insert(id.clone()))
                .collect::<Vec<_>>();
            let response = json!({"id": id, "type": "response", "ok": true, "result": {"queued": fresh.len()}});
            self.native_output
                .send(response)
                .map_err(|_| "Rust backend is not available".to_owned())?;
            drop(pending);
            if !fresh.is_empty() {
                let sender = self.native_output.clone();
                let pending = Arc::clone(&self.thumbnail_pending);
                thread::spawn(move || {
                    let (watchlist, cache) = thumbnails::default_paths();
                    let data = thumbnails::cache_requested(&fresh, &watchlist, &cache);
                    let _ = sender
                        .send(json!({"type":"event", "event":"thumbnails.updated", "data":data}));
                    if let Ok(mut pending) = pending.lock() {
                        for id in fresh {
                            pending.remove(&id);
                        }
                    }
                });
            }
            return Ok(id);
        }
        if native::handles(command) {
            if matches!(command, "library.scan" | "services.check") {
                let sender = self.native_output.clone();
                let response_id = id.clone();
                let command = command.to_owned();
                thread::spawn(move || {
                    let response = native_response(&response_id, &command, &params);
                    let _ = sender.send(response);
                });
            } else {
                let response = native_response(&id, command, &params);
                self.native_output
                    .send(response)
                    .map_err(|_| "Rust backend is not available".to_string())?;
            }
            return Ok(id);
        }
        if self.input.is_none() {
            self.start_python()?;
        }
        self.input
            .as_ref()
            .ok_or("Python service is not available")?
            .send(json!({"id":id,"command":command,"params":params}))
            .map_err(|_| "Python service is not available".to_string())?;
        Ok(id)
    }

    pub fn drain(&self) -> Vec<Value> {
        self.output.try_iter().collect()
    }
}

fn native_response(id: &str, command: &str, params: &Value) -> Value {
    match native::dispatch(command, params) {
        Ok(result) => json!({"id": id, "type":"response", "ok":true, "result":result}),
        Err(message) => {
            let code = if matches!(command, "config.save" | "spotify.set_client_id") {
                "invalid_request"
            } else {
                "operation_failed"
            };
            json!({"id": id, "type":"response", "ok":false, "error":{"code":code, "message":message}})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Bridge;
    use serde_json::{json, Value};
    use std::collections::HashSet;
    use std::fs;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[test]
    fn library_scan_uses_rust_without_sending_a_python_request(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join("Track [dQw4w9WgXcQ].mp3"), b"audio")?;
        let (input, python_requests) = mpsc::channel::<Value>();
        let (native_output, output) = mpsc::channel::<Value>();
        let mut bridge = Bridge {
            input: Some(input),
            output,
            native_output,
            thumbnail_pending: Arc::new(Mutex::new(HashSet::new())),
            next_id: 1,
        };
        let id = bridge.send("library.scan", json!({"output": dir.path()}))?;
        let response = bridge.output.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(response["id"], id);
        assert_eq!(response["result"]["total_size"], "5.0 B");
        assert!(python_requests.try_recv().is_err());
        Ok(())
    }

    #[test]
    fn startup_answers_hello_without_starting_python() -> Result<(), Box<dyn std::error::Error>> {
        let mut bridge = Bridge::start()?;
        assert!(bridge.input.is_none());
        let id = bridge.send("hello", json!({}))?;
        let response = bridge.output.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(response["id"], id);
        assert_eq!(response["result"]["protocol_version"], 1);
        assert!(bridge.input.is_none());
        Ok(())
    }

    #[test]
    fn thumbnail_requests_get_a_native_protocol_response() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut bridge = Bridge::start()?;
        let id = bridge.send("thumbnails.cache", json!({"video_ids": []}))?;
        let response = bridge.output.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(response["id"], id);
        assert_eq!(response["result"]["queued"], 0);
        assert!(bridge.input.is_none());

        let id = bridge.send("thumbnails.cache", json!({"video_ids": [42]}))?;
        let response = bridge.output.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(response["id"], id);
        assert_eq!(response["error"]["code"], "invalid_request");
        assert!(bridge.input.is_none());
        Ok(())
    }
}
