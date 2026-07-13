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
