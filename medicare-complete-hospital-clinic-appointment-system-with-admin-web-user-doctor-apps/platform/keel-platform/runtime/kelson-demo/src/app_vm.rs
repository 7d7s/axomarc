//! MVA-004: the reference application's OWN view models — the first live
//! MVVC consumers in the runtime. The book page is declared end to end:
//! the appointments manifest route names the view AND the model
//! (`view = "pages.book-appointment"`, `view_model = "BookAppointmentVm"`,
//! MVA-005), the boot registers the binding below (and refuses boot if a
//! declaration points at nothing), and the template reads only through the
//! `@vm` binder.
//!
//! MVA-008 adds [`WaitingRoomVm`]: the live view's state. Its binding is
//! the FIRST one registered with [`kelson_views::ViewBinding::live`] —
//! opt-in per view, so the live loop can only ever attach to this page.

use kelson_views::{FieldKind, FieldSchema, ViewModel};

/// The book page's model: the availability counts for the queried day.
/// The declared page's operation result rides in under the reserved `vm`
/// namespace; the binder checks every read against this schema.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BookAppointmentVm {
    pub date: String,
    pub appointments_booked: u64,
}

impl ViewModel for BookAppointmentVm {
    const VIEW: &'static str = "pages.book-appointment";
    fn schema() -> &'static [FieldSchema] {
        kelson_views::view_schema![
            FieldSchema::required("date", FieldKind::String),
            FieldSchema::required("appointments_booked", FieldKind::Int),
        ]
    }
}

/// MVA-008: the live waiting-room's state — the state IS this view
/// model. A session holds one of these; every applied `change` advances
/// the session's revision and the wire publishes the diff of exactly
/// this declared shape.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WaitingRoomVm {
    pub waiting: u64,
}

impl ViewModel for WaitingRoomVm {
    const VIEW: &'static str = "pages.mvc-live";
    fn schema() -> &'static [FieldSchema] {
        kelson_views::view_schema![
            FieldSchema::required("waiting", FieldKind::Int),
        ]
    }
}
