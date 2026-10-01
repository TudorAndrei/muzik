use muzik_agent::Outcome;
use muzik_core::DecisionKind;
use serde_json::Value;

pub trait Chooser: Send + Sync {
    fn choose(&self, kind: DecisionKind, payload: &Value, model: &str) -> Result<Outcome, String>;
}

pub struct Codex;

impl Chooser for Codex {
    fn choose(&self, kind: DecisionKind, payload: &Value, model: &str) -> Result<Outcome, String> {
        muzik_agent::decide(kind, payload, model)
    }
}
