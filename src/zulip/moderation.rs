use crate::config::{CommandsConfig, PocketBaseConfig};

/// Base URL of the public calendar site that publishes approved events.
const EVENTS_SITE_URL: &str = "https://kalenteri.palikkaharrastajat.fi";
/// Base URL of the public map site that publishes approved locations.
const LOCATIONS_SITE_URL: &str = "https://kartta.palikkaharrastajat.fi";

/// Which collection a Zulip moderation topic belongs to.
///
/// Every part of the moderation flow that differs between events and locations — the target
/// collection, the localization keys, the public URL — is resolved through this enum so the
/// handler itself stays single-pathed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModerationKind {
    Event,
    Location,
}

/// Topic prefix marking a newly proposed event.
pub const TOPIC_NEW_EVENT: &str = "🆕";
/// Topic prefix marking a proposed edit to an existing event.
pub const TOPIC_EDIT_EVENT: &str = "📝";
/// Topic prefix marking a newly proposed location.
pub const TOPIC_NEW_LOCATION: &str = "📍";
/// Topic prefix marking a proposed edit to an existing location.
pub const TOPIC_EDIT_LOCATION: &str = "📌";

impl ModerationKind {
    /// Classifies a Zulip topic, returning the kind and whether it is an edit proposal.
    ///
    /// Returns `None` for topics that are not moderation topics at all.
    #[must_use]
    pub fn from_topic(subject: &str) -> Option<(Self, bool)> {
        if subject.starts_with(TOPIC_NEW_EVENT) {
            Some((Self::Event, false))
        } else if subject.starts_with(TOPIC_EDIT_EVENT) {
            Some((Self::Event, true))
        } else if subject.starts_with(TOPIC_NEW_LOCATION) {
            Some((Self::Location, false))
        } else if subject.starts_with(TOPIC_EDIT_LOCATION) {
            Some((Self::Location, true))
        } else {
            None
        }
    }

    /// Topic prefix to use when rebuilding a topic of this kind.
    #[must_use]
    pub const fn topic_prefix(self, is_edit: bool) -> &'static str {
        match (self, is_edit) {
            (Self::Event, false) => TOPIC_NEW_EVENT,
            (Self::Event, true) => TOPIC_EDIT_EVENT,
            (Self::Location, false) => TOPIC_NEW_LOCATION,
            (Self::Location, true) => TOPIC_EDIT_LOCATION,
        }
    }

    /// `PocketBase` collection approved records of this kind are written to.
    #[must_use]
    pub fn collection(self, pb: &PocketBaseConfig) -> &str {
        match self {
            Self::Event => &pb.collection,
            Self::Location => pb.locations_collection(),
        }
    }

    /// Message sent to the submitter when their proposal is approved.
    #[must_use]
    pub const fn approved_key(self, is_edit: bool) -> &'static str {
        match (self, is_edit) {
            (Self::Event, false) => "command.events.submission_approved",
            (Self::Event, true) => "command.events.edit_approved",
            (Self::Location, false) => "command.locations.submission_approved",
            (Self::Location, true) => "command.locations.edit_approved",
        }
    }

    /// Message sent to the submitter when their proposal is rejected.
    #[must_use]
    pub const fn rejected_key(self) -> &'static str {
        match self {
            Self::Event => "command.events.submission_rejected",
            Self::Location => "command.locations.submission_rejected",
        }
    }

    /// Message forwarded to the submitter when a moderator asks them a question.
    #[must_use]
    pub const fn moderator_asked_key(self) -> &'static str {
        match self {
            Self::Event => "command.zulip.moderator_asked",
            Self::Location => "command.zulip.moderator_asked_location",
        }
    }

    /// Confirmation sent when the submitter's Discord DM reply reaches the moderators.
    #[must_use]
    pub const fn dm_reply_sent_key(self) -> &'static str {
        match self {
            Self::Event => "command.events.dm_reply_sent",
            Self::Location => "command.locations.dm_reply_sent",
        }
    }

    /// Zulip template for a freshly proposed record.
    #[must_use]
    pub const fn new_template_key(self, has_image: bool) -> &'static str {
        match (self, has_image) {
            (Self::Event, true) => "command.zulip.new_event",
            (Self::Event, false) => "command.zulip.new_event_no_image",
            (Self::Location, true) => "command.zulip.new_location",
            (Self::Location, false) => "command.zulip.new_location_no_image",
        }
    }

    /// Zulip template for a proposed edit.
    #[must_use]
    pub const fn edit_template_key(self) -> &'static str {
        match self {
            Self::Event => "command.zulip.edit_event",
            Self::Location => "command.zulip.edit_location",
        }
    }

    /// Zulip confirmation posted after a successful push, when a record URL is known.
    #[must_use]
    pub const fn publish_success_key(self) -> &'static str {
        match self {
            Self::Event => "command.zulip.publish_success_with_url",
            Self::Location => "command.zulip.publish_success_with_url_location",
        }
    }

    /// Public URL of an approved record.
    #[must_use]
    pub fn public_url(self, id: &str) -> String {
        match self {
            Self::Event => format!("{EVENTS_SITE_URL}/#/events/{id}"),
            Self::Location => format!("{LOCATIONS_SITE_URL}/#/locations/{id}"),
        }
    }

    /// Zulip error message sent when pushing to `PocketBase` fails.
    #[must_use]
    pub const fn push_failed_key(self) -> &'static str {
        match self {
            Self::Event => "command.zulip.push_failed",
            Self::Location => "command.zulip.push_failed_location",
        }
    }

    /// Whether the bot should @-mention the submitter when a DM cannot be delivered.
    #[must_use]
    pub const fn enable_fallback_mention(self, commands: &CommandsConfig) -> bool {
        match self {
            Self::Event => commands.events.enable_fallback_mention,
            Self::Location => commands.locations.enable_fallback_mention,
        }
    }

    /// Renders the moderator-facing YAML block for a stored payload.
    #[must_use]
    pub fn display_yaml(self, payload: &serde_json::Value) -> String {
        match self {
            Self::Event => crate::workflows::events::modals::generate_display_yaml(payload),
            Self::Location => {
                crate::workflows::locations::modals::generate_location_display_yaml(payload)
            }
        }
    }

    /// Renders a unified diff between the live record and the proposed YAML.
    pub async fn diff_str(
        self,
        pb_cfg: Option<&PocketBaseConfig>,
        http: &dyn crate::http::HttpProvider,
        limit: u64,
        uid: &str,
        new_yaml: &str,
    ) -> String {
        match self {
            Self::Event => {
                crate::workflows::events::modals::generate_diff_str(
                    pb_cfg, http, limit, uid, new_yaml,
                )
                .await
            }
            Self::Location => {
                crate::workflows::locations::modals::generate_location_diff_str(
                    pb_cfg, http, limit, uid, new_yaml,
                )
                .await
            }
        }
    }

    /// Rebuilds the canonical topic for a payload after a moderator edits it.
    #[must_use]
    pub fn format_topic(self, is_edit: bool, title: &str, start_date: &str, uid: &str) -> String {
        let prefix = self.topic_prefix(is_edit);
        match self {
            Self::Event => {
                crate::workflows::events::format_zulip_topic(prefix, start_date, title, uid)
            }
            Self::Location => {
                crate::workflows::locations::format_location_zulip_topic(prefix, title, uid)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_topic() {
        assert_eq!(
            ModerationKind::from_topic("🆕 2026-07-14 Test 🆔 123"),
            Some((ModerationKind::Event, false))
        );
        assert_eq!(
            ModerationKind::from_topic("📝 2026-07-14 Test 🆔 123"),
            Some((ModerationKind::Event, true))
        );
        assert_eq!(
            ModerationKind::from_topic("📍 Legokauppa 🆔 123"),
            Some((ModerationKind::Location, false))
        );
        assert_eq!(
            ModerationKind::from_topic("📌 Legokauppa 🆔 123"),
            Some((ModerationKind::Location, true))
        );
        assert_eq!(ModerationKind::from_topic("general chat"), None);
        assert_eq!(ModerationKind::from_topic(""), None);
    }

    #[test]
    fn test_topic_prefix_round_trips() {
        for kind in [ModerationKind::Event, ModerationKind::Location] {
            for is_edit in [false, true] {
                let prefix = kind.topic_prefix(is_edit);
                assert_eq!(
                    ModerationKind::from_topic(&format!("{prefix} whatever")),
                    Some((kind, is_edit))
                );
            }
        }
    }

    #[test]
    fn test_collection() {
        let mut pb = PocketBaseConfig {
            url: "https://pb.example.com".to_string(),
            collection: "events".to_string(),
            locations_collection: None,
        };
        assert_eq!(ModerationKind::Event.collection(&pb), "events");
        assert_eq!(ModerationKind::Location.collection(&pb), "locations");

        pb.locations_collection = Some("map_places".to_string());
        assert_eq!(ModerationKind::Location.collection(&pb), "map_places");
    }

    #[test]
    fn test_public_url_uses_the_right_host() {
        assert_eq!(
            ModerationKind::Event.public_url("abc123"),
            "https://kalenteri.palikkaharrastajat.fi/#/events/abc123"
        );
        assert_eq!(
            ModerationKind::Location.public_url("abc123"),
            "https://kartta.palikkaharrastajat.fi/#/locations/abc123"
        );
    }

    #[test]
    fn test_localization_keys() {
        assert_eq!(
            ModerationKind::Event.approved_key(false),
            "command.events.submission_approved"
        );
        assert_eq!(
            ModerationKind::Location.approved_key(true),
            "command.locations.edit_approved"
        );
        assert_eq!(
            ModerationKind::Location.rejected_key(),
            "command.locations.submission_rejected"
        );
        assert_eq!(
            ModerationKind::Location.new_template_key(true),
            "command.zulip.new_location"
        );
        assert_eq!(
            ModerationKind::Location.new_template_key(false),
            "command.zulip.new_location_no_image"
        );
        assert_eq!(
            ModerationKind::Event.new_template_key(false),
            "command.zulip.new_event_no_image"
        );
        assert_eq!(
            ModerationKind::Location.edit_template_key(),
            "command.zulip.edit_location"
        );
        assert_eq!(
            ModerationKind::Location.publish_success_key(),
            "command.zulip.publish_success_with_url_location"
        );
        assert_eq!(
            ModerationKind::Location.moderator_asked_key(),
            "command.zulip.moderator_asked_location"
        );
        assert_eq!(
            ModerationKind::Location.dm_reply_sent_key(),
            "command.locations.dm_reply_sent"
        );
    }

    #[test]
    fn test_format_topic() {
        assert_eq!(
            ModerationKind::Event.format_topic(false, "Test", "2026-07-14 10:00", "123"),
            "🆕 2026-07-14 Test 🆔 123"
        );
        // Locations carry no date, so it is left out of the topic entirely.
        assert_eq!(
            ModerationKind::Location.format_topic(true, "Legokauppa", "", "123"),
            "📌 Legokauppa 🆔 123"
        );
    }

    #[test]
    fn test_enable_fallback_mention_reads_the_matching_command_config() {
        let mut commands = CommandsConfig::default();
        commands.events.enable_fallback_mention = true;
        commands.locations.enable_fallback_mention = false;

        assert!(ModerationKind::Event.enable_fallback_mention(&commands));
        assert!(!ModerationKind::Location.enable_fallback_mention(&commands));
    }
}
