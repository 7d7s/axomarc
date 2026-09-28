//! Appointment state machine (APT-01). Exhaustive; illegal paths rejected.
use kernel_workflow::{StateMachine, Transition};

pub fn machine() -> StateMachine {
    StateMachine {
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
    }
}