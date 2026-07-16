pub mod bricklink {
    /// Returns the Bricklink catalog URL for a given set ID (e.g., "42083-1").
    pub fn set_url(set_id: &str) -> String {
        format!("https://www.bricklink.com/v2/catalog/catalogitem.page?S={set_id}")
    }

    /// Returns the Bricklink catalog URL for a given part ID.
    pub fn part_url(part_id: &str) -> String {
        format!("https://www.bricklink.com/v2/catalog/catalogitem.page?P={part_id}")
    }
}

pub mod lego {
    /// Returns the LEGO.com search URL for a given query (typically a set number).
    pub fn search_url(query: &str) -> String {
        format!("https://www.lego.com/fi-fi/search?q={query}")
    }
}

pub mod brickset {
    pub fn set_url(set_id: &str) -> String {
        format!("https://brickset.com/sets/{set_id}")
    }
}

pub mod rebrickable {
    pub fn set_url(set_id: &str) -> String {
        format!("https://rebrickable.com/sets/{set_id}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bricklink_urls() {
        assert_eq!(
            bricklink::set_url("42083-1"),
            "https://www.bricklink.com/v2/catalog/catalogitem.page?S=42083-1"
        );
        assert_eq!(
            bricklink::part_url("3001"),
            "https://www.bricklink.com/v2/catalog/catalogitem.page?P=3001"
        );
    }

    #[test]
    fn test_lego_search_url() {
        assert_eq!(
            lego::search_url("42083"),
            "https://www.lego.com/fi-fi/search?q=42083"
        );
    }

    #[test]
    fn test_brickset_url() {
        assert_eq!(
            brickset::set_url("42083-1"),
            "https://brickset.com/sets/42083-1"
        );
    }

    #[test]
    fn test_rebrickable_url() {
        assert_eq!(
            rebrickable::set_url("42083-1"),
            "https://rebrickable.com/sets/42083-1"
        );
    }
}
