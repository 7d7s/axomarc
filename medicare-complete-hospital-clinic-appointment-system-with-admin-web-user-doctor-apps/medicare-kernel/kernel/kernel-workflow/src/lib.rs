//! Workflow kernel: exhaustive transition tables; illegal transitions are
//! rejected with workflow.invalid_transition (KER-04). Pure + unit-testable.
use kernel_core::KernelError;

#[derive(Debug, Clone, Copy)]
pub struct Transition {
    pub from: &'static str,
    pub event: &'static str,
    pub to: &'static str,
}

pub struct StateMachine {
    pub name: &'static str,
    pub initial: &'static str,
    pub transitions: &'static [Transition],
}

impl StateMachine {
    pub fn next(&self, state: &str, event: &str) -> Result<&'static str, KernelError> {
        self.transitions
            .iter()
            .find(|t| t.from == state && t.event == event)
            .map(|t| t.to)
            .ok_or(KernelError::WorkflowInvalidTransition)
    }

    pub fn allowed_events(&self, state: &str) -> Vec<&'static str> {
        self.transitions
            .iter()
            .filter(|t| t.from == state)
            .map(|t| t.event)
            .collect()
    }

    pub fn is_terminal(&self, state: &str) -> bool {
        self.allowed_events(state).is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APT: StateMachine = StateMachine {
        name: "appointment",
        initial: "SCHEDULED",
        transitions: &[
            Transition { from: "SCHEDULED", event: "check_in", to: "CHECKED_IN" },
            Transition { from: "CHECKED_IN", event: "start_consult", to: "IN_CONSULTATION" },
            Transition { from: "IN_CONSULTATION", event: "complete", to: "COMPLETED" },
            Transition { from: "SCHEDULED", event: "no_show", to: "NO_SHOW" },
            Transition { from: "SCHEDULED", event: "cancel_request", to: "CANCEL_REQUESTED" },
            Transition { from: "CANCEL_REQUESTED", event: "cancel_approve", to: "CANCELLED" },
            Transition { from: "CANCEL_REQUESTED", event: "cancel_decline", to: "SCHEDULED" },
        ],
    };

    #[test]
    fn happy_path() {
        assert_eq!(APT.next("SCHEDULED", "check_in").unwrap(), "CHECKED_IN");
        assert_eq!(APT.next("CHECKED_IN", "start_consult").unwrap(), "IN_CONSULTATION");
        assert_eq!(APT.next("IN_CONSULTATION", "complete").unwrap(), "COMPLETED");
    }

    #[test]
    fn illegal_paths_rejected() {
        assert!(APT.next("SCHEDULED", "complete").is_err());
        assert!(APT.next("COMPLETED", "check_in").is_err());
        assert!(APT.next("NO_SHOW", "cancel_request").is_err());
    }

    #[test]
    fn cancellation_side_path() {
        assert_eq!(APT.next("SCHEDULED", "cancel_request").unwrap(), "CANCEL_REQUESTED");
        assert_eq!(APT.next("CANCEL_REQUESTED", "cancel_approve").unwrap(), "CANCELLED");
        assert_eq!(APT.next("CANCEL_REQUESTED", "cancel_decline").unwrap(), "SCHEDULED");
    }

    #[test]
    fn allowed_events_drive_client_buttons() {
        let ev = APT.allowed_events("SCHEDULED");
        assert!(ev.contains(&"check_in"));
        assert!(ev.contains(&"cancel_request"));
        assert!(APT.is_terminal("COMPLETED"));
    }
}