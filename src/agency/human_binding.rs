//! Structured Telegram ↔ agent binding map for the human-onboarding handshake.
//!
//! Tier-A item R21/R22 from `docs/03-gap-analysis-refresh.md` (family-team gap
//! analysis). The multi-bot Telegram work (`src/notify/telegram.rs`) landed
//! per-human bot config, and `wg agency human add` (see
//! `src/commands/agency_human.rs`) is the one-command onboarding wrapper. What
//! was missing — flagged as R22 — is a *structured artifact* recording which
//! Telegram user maps to which agency agent, and whether that human has
//! confirmed joining via the `YES` handshake. This module is that artifact.
//!
//! The map is persisted as YAML at `<agency_dir>/bindings/telegram.yaml`:
//!
//! ```yaml
//! bindings:
//!   - telegram_user: "78901234"
//!     agent_id: human-nadin
//!     name: Nadin
//!     bot_id: nadin
//!     confirmed: true
//!     created_at: "2026-07-10T12:00:00Z"
//!     confirmed_at: "2026-07-10T12:03:11Z"
//! ```
//!
//! One human ↔ one agent is enforced on [`TelegramBindingMap::add`]: a given
//! Telegram user (or a given agent id) may appear at most once.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::atomic_file::write_atomic;

use super::store::AgencyError;

/// A single Telegram-user → agency-agent binding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelegramBinding {
    /// The Telegram user id (numeric) or `@handle` as supplied to
    /// `wg agency human add --telegram <...>`. Matched verbatim against the
    /// `sender` of inbound messages (Telegram numeric ids are stable; handles
    /// are convenience aliases the operator typed).
    pub telegram_user: String,
    /// Agency agent id this human maps to (the `is_human` agent created by
    /// `wg agency human add`).
    pub agent_id: String,
    /// Human-readable name.
    pub name: String,
    /// Which configured bot fronts this human (the `bot_id` key from
    /// `[telegram.bots.<id>]`). `None` when no bot was configured at add time
    /// (the manual-fallback path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<String>,
    /// Whether the human confirmed joining via the `YES` handshake.
    #[serde(default)]
    pub confirmed: bool,
    /// When the binding was first recorded.
    pub created_at: DateTime<Utc>,
    /// When the human confirmed (set by [`apply_confirmation`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_at: Option<DateTime<Utc>>,
}

impl TelegramBinding {
    /// Create a fresh, unconfirmed binding stamped at `created_at`.
    pub fn new(
        telegram_user: impl Into<String>,
        agent_id: impl Into<String>,
        name: impl Into<String>,
        bot_id: Option<String>,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            telegram_user: telegram_user.into(),
            agent_id: agent_id.into(),
            name: name.into(),
            bot_id,
            confirmed: false,
            created_at,
            confirmed_at: None,
        }
    }
}

/// The full set of Telegram bindings, serialized as one YAML document.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TelegramBindingMap {
    #[serde(default)]
    pub bindings: Vec<TelegramBinding>,
}

/// Path of the binding artifact relative to the agency store root.
pub fn binding_path(agency_dir: &Path) -> PathBuf {
    agency_dir.join("bindings").join("telegram.yaml")
}

impl TelegramBindingMap {
    /// Load the binding map from `<agency_dir>/bindings/telegram.yaml`.
    ///
    /// Returns an empty map when the file does not exist yet (first onboard).
    pub fn load(agency_dir: &Path) -> Result<Self, AgencyError> {
        let path = binding_path(agency_dir);
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(&path)?;
        if contents.trim().is_empty() {
            return Ok(Self::default());
        }
        Ok(serde_yaml::from_str(&contents)?)
    }

    /// Persist the binding map atomically.
    pub fn save(&self, agency_dir: &Path) -> Result<PathBuf, AgencyError> {
        let path = binding_path(agency_dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let yaml = serde_yaml::to_string(self)?;
        write_atomic(&path, yaml.as_bytes())?;
        Ok(path)
    }

    /// Find a binding by Telegram user id/handle.
    pub fn find_by_user(&self, telegram_user: &str) -> Option<&TelegramBinding> {
        self.bindings
            .iter()
            .find(|b| b.telegram_user == telegram_user)
    }

    /// Find a binding by agency agent id.
    pub fn find_by_agent(&self, agent_id: &str) -> Option<&TelegramBinding> {
        self.bindings.iter().find(|b| b.agent_id == agent_id)
    }

    /// Add a binding, enforcing one-human-one-agent.
    ///
    /// Errors if the Telegram user is already bound, or if the agent id is
    /// already bound to a (possibly different) Telegram user.
    pub fn add(&mut self, binding: TelegramBinding) -> Result<(), AgencyError> {
        if let Some(existing) = self.find_by_user(&binding.telegram_user) {
            return Err(AgencyError::Ambiguous(format!(
                "Telegram user '{}' is already bound to agent '{}'",
                binding.telegram_user, existing.agent_id
            )));
        }
        if let Some(existing) = self.find_by_agent(&binding.agent_id) {
            return Err(AgencyError::Ambiguous(format!(
                "agent '{}' is already bound to Telegram user '{}'",
                binding.agent_id, existing.telegram_user
            )));
        }
        self.bindings.push(binding);
        Ok(())
    }
}

/// Returns `true` when `body` is an affirmative `YES` reply.
///
/// Case-insensitive, whitespace-trimmed. Accepts `yes` and the shorthand `y`
/// (the vision doc's handshake is "reply YES", but a bare `y` is the obvious
/// human variant and costs nothing to honour). Deliberately strict otherwise —
/// a message like "yes please, but who is this?" is NOT treated as a
/// confirmation so an ambiguous reply doesn't silently bind someone.
pub fn is_affirmative(body: &str) -> bool {
    let normalized = body.trim().to_ascii_lowercase();
    normalized == "yes" || normalized == "y"
}

/// Apply an inbound reply to the binding map, confirming the sender if the
/// message is an affirmative handshake reply.
///
/// This is the pure, unit-testable core of the inbound-listener hook. Given
/// the `sender` (Telegram user id) and message `body`, if the sender has an
/// unconfirmed binding and the body [`is_affirmative`], the binding is marked
/// confirmed at `at` and the human's name is returned. Returns `None` when the
/// sender is unknown, already confirmed, or the reply is not affirmative —
/// making it safe to call on every inbound message and idempotent on repeats.
pub fn apply_confirmation(
    map: &mut TelegramBindingMap,
    sender: &str,
    body: &str,
    at: DateTime<Utc>,
) -> Option<String> {
    if !is_affirmative(body) {
        return None;
    }
    let binding = map
        .bindings
        .iter_mut()
        .find(|b| b.telegram_user == sender && !b.confirmed)?;
    binding.confirmed = true;
    binding.confirmed_at = Some(at);
    Some(binding.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn ts() -> DateTime<Utc> {
        "2026-07-10T12:00:00Z".parse().unwrap()
    }

    fn binding(user: &str, agent: &str, name: &str) -> TelegramBinding {
        TelegramBinding::new(user, agent, name, Some("botx".to_string()), ts())
    }

    #[test]
    fn test_add_and_find() {
        let mut map = TelegramBindingMap::default();
        map.add(binding("111", "human-nadin", "Nadin")).unwrap();

        assert_eq!(map.find_by_user("111").unwrap().name, "Nadin");
        assert_eq!(map.find_by_agent("human-nadin").unwrap().telegram_user, "111");
        assert!(map.find_by_user("999").is_none());
    }

    #[test]
    fn test_one_human_one_agent_rejects_duplicate_user() {
        let mut map = TelegramBindingMap::default();
        map.add(binding("111", "human-a", "A")).unwrap();
        // Same telegram user, different agent → rejected.
        let err = map.add(binding("111", "human-b", "B")).unwrap_err();
        assert!(err.to_string().contains("already bound"));
        assert_eq!(map.bindings.len(), 1);
    }

    #[test]
    fn test_one_human_one_agent_rejects_duplicate_agent() {
        let mut map = TelegramBindingMap::default();
        map.add(binding("111", "human-a", "A")).unwrap();
        // Different telegram user, same agent → rejected.
        let err = map.add(binding("222", "human-a", "A")).unwrap_err();
        assert!(err.to_string().contains("already bound"));
        assert_eq!(map.bindings.len(), 1);
    }

    #[test]
    fn test_save_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let agency_dir = tmp.path();

        let mut map = TelegramBindingMap::default();
        map.add(binding("111", "human-nadin", "Nadin")).unwrap();
        map.add(binding("222", "human-erik", "Erik")).unwrap();
        let path = map.save(agency_dir).unwrap();
        assert!(path.exists());

        let loaded = TelegramBindingMap::load(agency_dir).unwrap();
        assert_eq!(loaded.bindings, map.bindings);
    }

    #[test]
    fn test_load_missing_is_empty() {
        let tmp = TempDir::new().unwrap();
        let loaded = TelegramBindingMap::load(tmp.path()).unwrap();
        assert!(loaded.bindings.is_empty());
    }

    #[test]
    fn test_is_affirmative() {
        assert!(is_affirmative("YES"));
        assert!(is_affirmative("yes"));
        assert!(is_affirmative("  Yes  "));
        assert!(is_affirmative("y"));
        assert!(is_affirmative("Y"));
        assert!(!is_affirmative("no"));
        assert!(!is_affirmative("yes please, but who is this?"));
        assert!(!is_affirmative(""));
        assert!(!is_affirmative("yeah"));
    }

    #[test]
    fn test_apply_confirmation_marks_confirmed() {
        let mut map = TelegramBindingMap::default();
        map.add(binding("111", "human-nadin", "Nadin")).unwrap();
        assert!(!map.find_by_user("111").unwrap().confirmed);

        let confirmed_at = "2026-07-10T12:03:11Z".parse().unwrap();
        let name = apply_confirmation(&mut map, "111", "YES", confirmed_at);
        assert_eq!(name, Some("Nadin".to_string()));

        let b = map.find_by_user("111").unwrap();
        assert!(b.confirmed);
        assert_eq!(b.confirmed_at, Some(confirmed_at));
    }

    #[test]
    fn test_apply_confirmation_ignores_unknown_sender() {
        let mut map = TelegramBindingMap::default();
        map.add(binding("111", "human-nadin", "Nadin")).unwrap();
        assert_eq!(apply_confirmation(&mut map, "999", "YES", ts()), None);
        assert!(!map.find_by_user("111").unwrap().confirmed);
    }

    #[test]
    fn test_apply_confirmation_ignores_non_affirmative() {
        let mut map = TelegramBindingMap::default();
        map.add(binding("111", "human-nadin", "Nadin")).unwrap();
        assert_eq!(apply_confirmation(&mut map, "111", "no thanks", ts()), None);
        assert!(!map.find_by_user("111").unwrap().confirmed);
    }

    #[test]
    fn test_apply_confirmation_idempotent() {
        let mut map = TelegramBindingMap::default();
        map.add(binding("111", "human-nadin", "Nadin")).unwrap();
        let first = "2026-07-10T12:03:11Z".parse().unwrap();
        assert_eq!(
            apply_confirmation(&mut map, "111", "yes", first),
            Some("Nadin".to_string())
        );
        // Second YES is a no-op: already confirmed, confirmed_at unchanged.
        let second = "2026-07-10T13:00:00Z".parse().unwrap();
        assert_eq!(apply_confirmation(&mut map, "111", "yes", second), None);
        assert_eq!(map.find_by_user("111").unwrap().confirmed_at, Some(first));
    }
}
