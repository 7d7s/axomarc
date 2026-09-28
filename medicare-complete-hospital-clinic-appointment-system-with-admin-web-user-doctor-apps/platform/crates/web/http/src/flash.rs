//! Flash data on sessions (S9): put-once / read-once values that survive
//! exactly one redirect. Stored under the `_flash.*` namespace inside the
//! session data bag; `flash_take` reads and removes atomically.

use keel_foundation::AppError;
use keel_session::{SessionId, SessionManager};
use serde_json::Value as Json;

const FLASH_PREFIX: &str = "_flash.";

/// Store a flash value for the next request (survives one read).
pub fn flash_put(
    manager: &SessionManager,
    id: &SessionId,
    key: &str,
    value: Json,
) -> Result<(), AppError> {
    manager.put(id, format!("{FLASH_PREFIX}{key}"), value)
}

/// Read a flash value once: present in this request, gone afterwards.
pub fn flash_take(
    manager: &SessionManager,
    id: &SessionId,
    key: &str,
) -> Result<Option<Json>, AppError> {
    let Some(mut session) = manager.store().load(id)? else {
        return Ok(None);
    };
    let flash_key = format!("{FLASH_PREFIX}{key}");
    let Some(value) = session.data.remove(&flash_key) else {
        return Ok(None);
    };
    manager.store().save(&session)?;
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel_foundation::PrincipalId;
    use keel_session::{InMemorySessionStore, SessionPolicy};
    use time::OffsetDateTime;

    fn manager() -> SessionManager {
        SessionManager::new(
            InMemorySessionStore::shared(),
            SessionPolicy::default(),
            b"flash-test-secret",
        )
    }

    #[test]
    fn flash_put_take_then_gone() {
        let manager = manager();
        let session = manager
            .start(PrincipalId::new(), None, OffsetDateTime::now_utc())
            .unwrap();

        flash_put(&manager, &session.id, "notice", serde_json::json!("saved!")).unwrap();
        assert_eq!(
            flash_take(&manager, &session.id, "notice").unwrap(),
            Some(serde_json::json!("saved!")),
            "first read returns the value"
        );
        assert_eq!(
            flash_take(&manager, &session.id, "notice").unwrap(),
            None,
            "second read is empty — flash survives exactly one take"
        );
    }

    #[test]
    fn flash_take_unknown_session_is_none() {
        let manager = manager();
        let ghost = SessionId::generate();
        assert_eq!(flash_take(&manager, &ghost, "x").unwrap(), None);
    }
}
