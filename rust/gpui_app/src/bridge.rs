//! JSON-lines link to the Python workflow process.
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

pub struct Bridge {
    input: Sender<Value>,
    output: Receiver<Value>,
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
            next_id: 1,
        })
    }

    pub fn send(&mut self, command: &str, params: Value) -> Result<String, String> {
        let id = self.next_id.to_string();
        self.next_id += 1;
        self.input
            .send(json!({"id":id,"command":command,"params":params}))
            .map_err(|_| "Python service is not available".to_string())?;
        Ok(id)
    }

    pub fn drain(&self) -> Vec<Value> {
        self.output.try_iter().collect()
    }
}
