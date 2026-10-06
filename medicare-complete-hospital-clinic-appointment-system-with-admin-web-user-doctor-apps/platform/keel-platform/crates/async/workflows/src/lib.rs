//! # keel-workflow
//! Workflows are executable metadata (11-SPEC §28/§33/§34): states, transitions,
//! guards, actions, compensation, permissions, timeout. The workflow compiler
//! rejects unreachable states, dead ends, unknown states and duplicate transitions.

use std::collections::{BTreeSet, VecDeque};

use keel_foundation::AppError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transition {
    pub from: String,
    pub to: String,
    pub event: String,
    pub guard: Option<String>,
    pub capability: Option<String>,
    pub compensation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDef {
    pub name: String,
    pub initial: String,
    #[serde(default)]
    pub terminal: BTreeSet<String>,
    pub states: BTreeSet<String>,
    pub transitions: Vec<Transition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowViolation {
    /// WF-UNKNOWN-STATE · WF-UNREACHABLE · WF-DEADEND · WF-DUP-TRANSITION · WF-INITIAL
    pub code: String,
    pub message: String,
}

impl WorkflowDef {
    /// Compile-time validation (G14). Returns ALL violations, not just the first.
    pub fn validate(&self) -> Result<(), Vec<WorkflowViolation>> {
        let mut violations = Vec::new();

        if !self.states.contains(&self.initial) {
            violations.push(WorkflowViolation {
                code: "WF-INITIAL".into(),
                message: format!(
                    "{}: initial state {} not in state set",
                    self.name, self.initial
                ),
            });
        }
        for t in &self.terminal {
            if !self.states.contains(t) {
                violations.push(WorkflowViolation {
                    code: "WF-UNKNOWN-STATE".into(),
                    message: format!("{}: terminal state {t} not in state set", self.name),
                });
            }
        }

        let mut seen_pairs = BTreeSet::new();
        for t in &self.transitions {
            if !self.states.contains(&t.from) || !self.states.contains(&t.to) {
                violations.push(WorkflowViolation {
                    code: "WF-UNKNOWN-STATE".into(),
                    message: format!(
                        "{}: transition {} -> {} references unknown state",
                        self.name, t.from, t.to
                    ),
                });
            }
            if !seen_pairs.insert((t.from.clone(), t.event.clone())) {
                violations.push(WorkflowViolation {
                    code: "WF-DUP-TRANSITION".into(),
                    message: format!(
                        "{}: duplicate transition from {} on {}",
                        self.name, t.from, t.event
                    ),
                });
            }
        }

        // reachability from initial
        let reachable = {
            let mut reach = BTreeSet::new();
            let mut queue = VecDeque::new();
            if self.states.contains(&self.initial) {
                reach.insert(self.initial.clone());
                queue.push_back(self.initial.clone());
            }
            while let Some(current) = queue.pop_front() {
                for t in &self.transitions {
                    if t.from == current
                        && self.states.contains(&t.to)
                        && reach.insert(t.to.clone())
                    {
                        queue.push_back(t.to.clone());
                    }
                }
            }
            reach
        };
        for s in &self.states {
            if !reachable.contains(s) {
                violations.push(WorkflowViolation {
                    code: "WF-UNREACHABLE".into(),
                    message: format!(
                        "{}: state {s} is unreachable from {}",
                        self.name, self.initial
                    ),
                });
            }
        }

        // dead ends: non-terminal reachable states need an outgoing transition
        for s in &reachable {
            if self.terminal.contains(s) {
                continue;
            }
            if !self.transitions.iter().any(|t| &t.from == s) {
                violations.push(WorkflowViolation {
                    code: "WF-DEADEND".into(),
                    message: format!(
                        "{}: non-terminal state {s} has no outgoing transition",
                        self.name
                    ),
                });
            }
        }

        if violations.is_empty() {
            Ok(())
        } else {
            Err(violations)
        }
    }
}

/// Runtime state machine over a validated definition.
#[derive(Debug, Clone)]
pub struct Machine {
    def: WorkflowDef,
    current: String,
}

impl Machine {
    pub fn start(def: WorkflowDef) -> Result<Self, AppError> {
        def.validate()
            .map_err(|v| AppError::workflow(format!("invalid workflow: {:?}", v)))?;
        Ok(Self {
            current: def.initial.clone(),
            def,
        })
    }

    pub fn current(&self) -> &str {
        &self.current
    }

    pub fn can(&self, event: &str) -> bool {
        self.def
            .transitions
            .iter()
            .any(|t| t.from == self.current && t.event == event)
    }

    /// Structural transition. Guards/capabilities are evaluated by the caller
    /// (through the execution engine); the machine enforces the graph only.
    pub fn apply(&mut self, event: &str) -> Result<String, AppError> {
        let t = self
            .def
            .transitions
            .iter()
            .find(|t| t.from == self.current && t.event == event)
            .ok_or_else(|| {
                AppError::workflow(format!(
                    "workflow {}: transition {} not allowed from {}",
                    self.def.name, event, self.current
                ))
            })?;
        self.current = t.to.clone();
        Ok(self.current.clone())
    }

    pub fn is_terminal(&self) -> bool {
        self.def.terminal.contains(&self.current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn appointment_flow() -> WorkflowDef {
        WorkflowDef {
            name: "appointment".into(),
            initial: "SCHEDULED".into(),
            states: [
                "SCHEDULED",
                "CHECKED_IN",
                "IN_CONSULTATION",
                "COMPLETED",
                "CANCELLED",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            terminal: ["COMPLETED", "CANCELLED"]
                .into_iter()
                .map(String::from)
                .collect(),
            transitions: vec![
                Transition {
                    from: "SCHEDULED".into(),
                    to: "CHECKED_IN".into(),
                    event: "check_in".into(),
                    guard: None,
                    capability: None,
                    compensation: None,
                },
                Transition {
                    from: "CHECKED_IN".into(),
                    to: "IN_CONSULTATION".into(),
                    event: "start".into(),
                    guard: None,
                    capability: None,
                    compensation: None,
                },
                Transition {
                    from: "IN_CONSULTATION".into(),
                    to: "COMPLETED".into(),
                    event: "finish".into(),
                    guard: None,
                    capability: None,
                    compensation: None,
                },
                Transition {
                    from: "SCHEDULED".into(),
                    to: "CANCELLED".into(),
                    event: "cancel".into(),
                    guard: None,
                    capability: None,
                    compensation: Some("release_slot".into()),
                },
            ],
        }
    }

    #[test]
    fn valid_workflow_compiles_and_transitions() {
        let mut m = Machine::start(appointment_flow()).unwrap();
        assert_eq!(m.current(), "SCHEDULED");
        m.apply("check_in").unwrap();
        m.apply("start").unwrap();
        m.apply("finish").unwrap();
        assert!(m.is_terminal());
    }

    #[test]
    fn illegal_transition_rejected() {
        let mut m = Machine::start(appointment_flow()).unwrap();
        assert!(m.apply("finish").is_err());
    }

    #[test]
    fn unreachable_state_detected() {
        let mut def = appointment_flow();
        def.states.insert("GHOST".into());
        let v = def.validate().unwrap_err();
        assert!(v.iter().any(|x| x.code == "WF-UNREACHABLE"));
    }

    #[test]
    fn dead_end_detected() {
        let mut def = appointment_flow();
        def.states.insert("STUCK".into());
        def.transitions.push(Transition {
            from: "SCHEDULED".into(),
            to: "STUCK".into(),
            event: "lose".into(),
            guard: None,
            capability: None,
            compensation: None,
        });
        let v = def.validate().unwrap_err();
        assert!(v.iter().any(|x| x.code == "WF-DEADEND"));
    }

    #[test]
    fn duplicate_transition_detected() {
        let mut def = appointment_flow();
        let dup = def.transitions[0].clone();
        def.transitions.push(dup);
        let v = def.validate().unwrap_err();
        assert!(v.iter().any(|x| x.code == "WF-DUP-TRANSITION"));
    }
}
