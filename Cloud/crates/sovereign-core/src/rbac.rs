// RBAC: Actor, Action, Role hierarchy, and enforcement.
// Lives in sovereign-core (no external dependencies) so use cases
// can call `rbac::check()` without depending on sovereign-auth.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::domain::UserRole;

/// Canonical actions in the Sovereign system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    AppDeploy,
    AppRollback,
    AppDelete,
    AppList,
    SecretRead,
    SecretSet,
    SecretRotate,
    ServiceInstall,
    ServiceRemove,
    ServiceRestart,
    PhpMyAdminView,
    FtpGrant,
    UserAdd,
    UserDisable,
    UserList,
    TokenCreate,
    TokenRevoke,
    ChatopsBind,
    ChatopsExec,
    BackupCreate,
    BackupList,
    DomainAdd,
    DomainRemove,
}

impl Action {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AppDeploy => "app.deploy",
            Self::AppRollback => "app.rollback",
            Self::AppDelete => "app.delete",
            Self::AppList => "app.list",
            Self::SecretRead => "secret.read",
            Self::SecretSet => "secret.set",
            Self::SecretRotate => "secret.rotate",
            Self::ServiceInstall => "service.install",
            Self::ServiceRemove => "service.remove",
            Self::ServiceRestart => "service.restart",
            Self::PhpMyAdminView => "phpmyadmin.view",
            Self::FtpGrant => "ftp.grant",
            Self::UserAdd => "user.add",
            Self::UserDisable => "user.disable",
            Self::UserList => "user.list",
            Self::TokenCreate => "token.create",
            Self::TokenRevoke => "token.revoke",
            Self::ChatopsBind => "chatops.bind",
            Self::ChatopsExec => "chatops.exec",
            Self::BackupCreate => "backup.create",
            Self::BackupList => "backup.list",
            Self::DomainAdd => "domain.add",
            Self::DomainRemove => "domain.remove",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "app.deploy" => Some(Self::AppDeploy),
            "app.rollback" => Some(Self::AppRollback),
            "app.delete" => Some(Self::AppDelete),
            "app.list" => Some(Self::AppList),
            "secret.read" => Some(Self::SecretRead),
            "secret.set" => Some(Self::SecretSet),
            "secret.rotate" => Some(Self::SecretRotate),
            "service.install" => Some(Self::ServiceInstall),
            "service.remove" => Some(Self::ServiceRemove),
            "service.restart" => Some(Self::ServiceRestart),
            "phpmyadmin.view" => Some(Self::PhpMyAdminView),
            "ftp.grant" => Some(Self::FtpGrant),
            "user.add" => Some(Self::UserAdd),
            "user.disable" => Some(Self::UserDisable),
            "user.list" => Some(Self::UserList),
            "token.create" => Some(Self::TokenCreate),
            "token.revoke" => Some(Self::TokenRevoke),
            "chatops.bind" => Some(Self::ChatopsBind),
            "chatops.exec" => Some(Self::ChatopsExec),
            "backup.create" => Some(Self::BackupCreate),
            "backup.list" => Some(Self::BackupList),
            "domain.add" => Some(Self::DomainAdd),
            "domain.remove" => Some(Self::DomainRemove),
            _ => None,
        }
    }
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The identity performing an action.
#[derive(Debug, Clone)]
pub enum Actor {
    /// The V0 single-tenant operator (bypasses RBAC — has Owner scope).
    Operator,
    /// A bound user with a role and optional scopes.
    User {
        id: String,
        role: UserRole,
        scopes: HashSet<Action>,
    },
    /// Internal system calls (same scope as Operator).
    System,
}

impl Actor {
    /// Parse an `actor: &str` (from audit events / V0 callers) into an Actor.
    /// "system" → System, anything else → Operator.
    pub fn from_str_loose(s: &str) -> Self {
        match s {
            "system" | "system:auto-rollback" => Actor::System,
            _ => Actor::Operator,
        }
    }

    pub fn id_str(&self) -> String {
        match self {
            Actor::Operator => "operator".to_string(),
            Actor::User { id, .. } => id.clone(),
            Actor::System => "system".to_string(),
        }
    }
}

/// RBAC error when access is denied.
#[derive(Debug, thiserror::Error)]
#[error("forbidden: actor {actor} cannot perform {action}")]
pub struct RbacError {
    pub actor: String,
    pub action: Action,
}

impl RbacError {
    pub fn forbidden(actor: &Actor, action: Action) -> Self {
        Self {
            actor: actor.id_str(),
            action,
        }
    }
}

/// Get the scope set for a UserRole.
pub fn role_scope_set(role: &UserRole) -> HashSet<Action> {
    use Action::*;
    match role {
        UserRole::Owner => HashSet::from_iter([
            AppDeploy, AppRollback, AppDelete, AppList,
            SecretRead, SecretSet, SecretRotate,
            ServiceInstall, ServiceRemove, ServiceRestart,
            PhpMyAdminView, FtpGrant,
            UserAdd, UserDisable, UserList,
            TokenCreate, TokenRevoke,
            ChatopsBind, ChatopsExec,
            BackupCreate, BackupList,
            DomainAdd, DomainRemove,
        ]),
        UserRole::Admin => HashSet::from_iter([
            AppDeploy, AppRollback, AppDelete, AppList,
            SecretRead, SecretSet, SecretRotate,
            ServiceInstall, ServiceRemove, ServiceRestart,
            PhpMyAdminView, FtpGrant,
            UserAdd, UserDisable, UserList,
            TokenCreate, TokenRevoke,
            ChatopsBind, ChatopsExec,
            BackupCreate, BackupList,
            DomainAdd, DomainRemove,
        ]),
        UserRole::Developer => HashSet::from_iter([
            AppDeploy, AppRollback, AppList,
            SecretRead, SecretSet,
            ServiceRestart,
            PhpMyAdminView,
            BackupCreate, BackupList,
            ChatopsBind, ChatopsExec,
        ]),
        UserRole::Readonly => HashSet::from_iter([
            AppList, SecretRead,
            PhpMyAdminView,
            BackupList, UserList,
            ChatopsBind,
        ]),
    }
}

/// Check whether an actor is allowed to perform an action.
pub fn check(actor: &Actor, action: Action) -> Result<(), RbacError> {
    let allowed = match actor {
        Actor::Operator | Actor::System => role_scope_set(&UserRole::Owner),
        Actor::User { role, scopes, .. } => {
            let mut combined = role_scope_set(role);
            combined.extend(scopes.iter().copied());
            combined
        }
    };
    if !allowed.contains(&action) {
        return Err(RbacError::forbidden(actor, action));
    }
    Ok(())
}

/// Parse a CSV scope string into a set of Actions.
pub fn parse_scopes(csv: &str) -> HashSet<Action> {
    csv.split(',')
        .filter_map(|s| Action::parse(s.trim()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_can_do_everything() {
        let actions = [
            Action::AppDeploy, Action::SecretSet, Action::UserAdd,
            Action::ServiceInstall, Action::TokenCreate, Action::DomainAdd,
        ];
        for a in actions {
            check(&Actor::Operator, a).unwrap();
        }
    }

    #[test]
    fn readonly_cannot_deploy() {
        let actor = Actor::User {
            id: "u1".to_string(),
            role: UserRole::Readonly,
            scopes: HashSet::new(),
        };
        let err = check(&actor, Action::AppDeploy).unwrap_err();
        assert!(err.to_string().contains("forbidden"));
    }

    #[test]
    fn developer_can_deploy() {
        let actor = Actor::User {
            id: "u1".to_string(),
            role: UserRole::Developer,
            scopes: HashSet::new(),
        };
        check(&actor, Action::AppDeploy).unwrap();
    }

    #[test]
    fn explicit_scope_overrides_role() {
        let mut scopes = HashSet::new();
        scopes.insert(Action::AppDeploy);
        let actor = Actor::User {
            id: "u1".to_string(),
            role: UserRole::Readonly,
            scopes,
        };
        check(&actor, Action::AppDeploy).unwrap();
    }

    #[test]
    fn parse_scopes_csv() {
        let scopes = parse_scopes("app.deploy,secret.read,bogus.action");
        assert!(scopes.contains(&Action::AppDeploy));
        assert!(scopes.contains(&Action::SecretRead));
        assert_eq!(scopes.len(), 2);
    }

    #[test]
    fn system_has_full_access() {
        check(&Actor::System, Action::AppDeploy).unwrap();
        check(&Actor::System, Action::UserAdd).unwrap();
    }
}
