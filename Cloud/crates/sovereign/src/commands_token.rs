// Token management commands: sovereign token create/list/revoke

use std::fmt::Write;

use sovereign_core::domain::Timestamp;
use sovereign_core::error::AppError;
use sovereign_core::ports::StoragePort;

use crate::cli::TokenCmd;
use crate::output::Output;

/// Hardcoded user ID for V0 single-tenant mode (owner).
const V0_OWNER_EMAIL: &str = "operator@localhost";

pub async fn dispatch(
    cmd: TokenCmd,
    store: &dyn StoragePort,
    _token: Option<&str>,
    out: &Output,
) -> Result<(), AppError> {
    let _ = _token;

    // Ensure the V0 operator user exists.
    let owner = match store.get_user_by_email(V0_OWNER_EMAIL).await? {
        Some(u) => u,
        None => {
            let new_user = sovereign_core::domain::NewUser {
                email: V0_OWNER_EMAIL.to_string(),
                role: sovereign_core::domain::UserRole::Owner,
            };
            store.create_user(new_user).await?
        }
    };

    match cmd {
        TokenCmd::Create {
            name,
            scopes,
            ttl,
        } => {
            let (token, hash) = sovereign_auth::token::generate_token();
            let expires_at = ttl
                .as_deref()
                .and_then(sovereign_auth::token::parse_ttl)
                .map(|secs| {
                    Timestamp::now() + sovereign_core::domain::Timestamp::from(secs as i64)
                });

            store
                .create_api_token(
                    owner.id,
                    &name,
                    &hash,
                    &scopes,
                    Timestamp::now(),
                    expires_at,
                )
                .await?;

            let mut buf = String::new();
            writeln!(&mut buf, "Token created: {name}").ok();
            writeln!(&mut buf, "Scopes: {scopes}").ok();
            writeln!(&mut buf).ok();
            writeln!(&mut buf, "SOVEREIGN_TOKEN={token}").ok();
            writeln!(&mut buf).ok();
            writeln!(&mut buf, "Save this token — it will not be shown again.").ok();
            let _ = out.text(&buf);
        }

        TokenCmd::List => {
            let tokens = store.list_api_tokens(owner.id).await?;
            if tokens.is_empty() {
                let _ = out.text("No API tokens.");
            } else {
                let mut buf = String::new();
                writeln!(&mut buf, "{:<20} {:<30}", "NAME", "SCOPES").ok();
                writeln!(&mut buf, "{}", "-".repeat(50)).ok();
                for t in &tokens {
                    writeln!(&mut buf, "{:<20} {:<30}", t.name, t.scopes).ok();
                }
                writeln!(&mut buf, "\n{} token(s) total.", tokens.len()).ok();
                let _ = out.text(&buf);
            }
        }

        TokenCmd::Revoke { name } => {
            store.delete_api_token(owner.id, &name).await?;
            let _ = out.text(&format!("Token revoked: {name}"));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn v0_owner_email_is_valid() {
        assert!(super::V0_OWNER_EMAIL.contains('@'));
    }
}
