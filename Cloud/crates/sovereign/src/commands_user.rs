// User management commands: sovereign user add/list/disable/enable/whoami

use std::fmt::Write;
use std::io::Write as IoWrite;

use sovereign_core::domain::{NewUser, Timestamp, UserRole};
use sovereign_core::error::AppError;
use sovereign_core::ports::StoragePort;

use crate::cli::UserCmd;
use crate::output::Output;

pub async fn dispatch(
    cmd: UserCmd,
    store: &dyn StoragePort,
    _token: Option<&str>,
    out: &Output,
) -> Result<(), AppError> {
    let _ = _token;

    match cmd {
        UserCmd::Add {
            email,
            password,
            role,
            name,
        } => {
            let existing = store.list_users().await?;
            let is_bootstrap = existing.is_empty();

            let user_role = if is_bootstrap {
                UserRole::Owner
            } else {
                parse_role(&role)?
            };

            let new_user = NewUser {
                email: email.clone(),
                role: user_role,
            };
            let user = store.create_user(new_user).await?;

            if let Some(display_name) = &name {
                store
                    .set_user_display_name(user.id, display_name)
                    .await?;
            }

            if password {
                eprint!("Password: ");
                std::io::stderr().flush().ok();
                let mut pw = String::new();
                std::io::stdin()
                    .read_line(&mut pw)
                    .map_err(|e| AppError::internal(format!("failed to read password: {e}")))?;
                let pw = pw.trim();
                if pw.is_empty() {
                    return Err(AppError::validation("password must not be empty"));
                }
                let hash = sovereign_auth::password::hash_password(pw)
                    .map_err(|e| AppError::internal(format!("password hash: {e}")))?;
                store
                    .set_user_password_hash(user.id, Some(&hash))
                    .await?;
            }

            if is_bootstrap {
                store.set_bootstrap_admin(user.id).await?;
                let _ = out.text(&format!(
                    "Bootstrap admin created: {email} (role: owner)\n\
                     This is the first user — they have full admin access."
                ));
            } else {
                let _ = out.text(&format!(
                    "User created: {email} (role: {user_role})"
                ));
            }
        }

        UserCmd::List { role } => {
            let users = store.list_users().await?;
            let filtered: Vec<_> = match role {
                Some(ref r) => {
                    let filter = parse_role(r)?;
                    users.into_iter().filter(|u| u.role == filter).collect()
                }
                None => users,
            };

            if filtered.is_empty() {
                let _ = out.text("No users found.");
            } else {
                let mut buf = String::new();
                writeln!(&mut buf, "{:<40} {:<12}", "EMAIL", "ROLE").ok();
                writeln!(&mut buf, "{}", "-".repeat(52)).ok();
                for u in &filtered {
                    writeln!(&mut buf, "{:<40} {:<12}", u.email, u.role).ok();
                }
                writeln!(&mut buf, "\n{} user(s) total.", filtered.len()).ok();
                let _ = out.text(&buf);
            }
        }

        UserCmd::Disable { email } => {
            let user = store
                .get_user_by_email(&email)
                .await?
                .ok_or_else(|| AppError::not_found("user"))?;
            store.disable_user(user.id, Timestamp::now()).await?;
            let _ = out.text(&format!("User disabled: {email}"));
        }

        UserCmd::Enable { email } => {
            let user = store
                .get_user_by_email(&email)
                .await?
                .ok_or_else(|| AppError::not_found("user"))?;
            store.enable_user(user.id).await?;
            let _ = out.text(&format!("User enabled: {email}"));
        }

        UserCmd::Whoami => {
            let _ = out.text(
                "Actor: operator (CLI)\n\
                 Role: owner\n\
                 Auth: V0 single-tenant (no token required)",
            );
        }
    }

    Ok(())
}

fn parse_role(s: &str) -> Result<UserRole, AppError> {
    match s.to_lowercase().as_str() {
        "owner" => Ok(UserRole::Owner),
        "admin" => Ok(UserRole::Admin),
        "developer" | "dev" => Ok(UserRole::Developer),
        "readonly" | "viewer" | "read" => Ok(UserRole::Readonly),
        _ => Err(AppError::validation(format!(
            "invalid role `{s}` — valid: owner, admin, developer, readonly"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_role_valid() {
        assert_eq!(parse_role("admin").unwrap(), UserRole::Admin);
        assert_eq!(parse_role("viewer").unwrap(), UserRole::Readonly);
        assert_eq!(parse_role("dev").unwrap(), UserRole::Developer);
        assert_eq!(parse_role("OWNER").unwrap(), UserRole::Owner);
    }

    #[test]
    fn parse_role_invalid() {
        assert!(parse_role("bogus").is_err());
    }
}
