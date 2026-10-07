use crate::Result;
use muzik_agent::Outcome;
use muzik_core::DecisionKind;
use serde_json::Value;

pub trait Chooser: Send + Sync {
    /// # Errors
    /// Returns an error when the agent cannot make a decision.
    fn choose(&self, kind: DecisionKind, payload: &Value, model: &str) -> Result<Outcome>;
}

pub struct Codex;

impl Chooser for Codex {
    fn choose(&self, kind: DecisionKind, payload: &Value, model: &str) -> Result<Outcome> {
        Ok(muzik_agent::decide(kind, payload, model)?)
    }
}
