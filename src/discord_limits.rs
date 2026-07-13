/// Discord UI limits to ensure robust API interactions.
pub const MAX_MODAL_INPUT: usize = 4000;
pub const MAX_MODAL_TITLE: usize = 45;
pub const MAX_BUTTON_LABEL: usize = 80;
pub const MAX_SELECT_OPTION_LABEL: usize = 100;
pub const MAX_MESSAGE_CONTENT: usize = 2000;

#[must_use]
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut truncated = s.chars().take(max.saturating_sub(3)).collect::<String>();
        truncated.push_str("...");
        truncated
    }
}
