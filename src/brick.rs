use serde::Deserialize;
use std::env;

#[derive(Deserialize, Debug)]
pub struct RebrickablePart {
    pub part_num: String,
    pub name: String,
    pub year_from: i32,
    pub year_to: i32,
    pub part_url: String,
    pub part_img_url: Option<String>,
    pub molds: Vec<String>,
    pub alternates: Vec<String>,
    pub print_of: Option<String>,
    pub external_ids: std::collections::HashMap<String, Vec<String>>,
}

#[derive(Deserialize, Debug)]
struct BricksetResponse {
    status: String,
    sets: Option<Vec<BricksetSet>>,
}

#[derive(Deserialize, Debug)]
#[allow(non_snake_case)]
pub struct BricksetSet {
    pub number: String,
    pub numberVariant: i32,
    pub name: String,
    pub year: i32,
    pub theme: String,
    pub subtheme: Option<String>,
    pub pieces: Option<i32>,
    pub image: Option<BricksetImage>,
    pub bricksetURL: Option<String>,
    pub rating: Option<f32>,
}

#[derive(Deserialize, Debug)]
#[allow(non_snake_case, dead_code)]
pub struct BricksetImage {
    pub thumbnailURL: Option<String>,
    pub imageURL: Option<String>,
}

pub async fn fetch_part(
    http: &dyn crate::http::HttpProvider,
    part_num: &str,
    limit: u64,
) -> anyhow::Result<RebrickablePart> {
    let api_key = env::var("REBRICKABLE_API_KEY")?;
    let url = format!("https://rebrickable.com/api/v3/lego/parts/{part_num}/");
    let res_text = http
        .get_json_with_auth(&url, Some(&format!("key {api_key}")), false, limit)
        .await?;
    let res: RebrickablePart = serde_json::from_str(&res_text)?;
    Ok(res)
}

pub async fn fetch_set(
    http: &dyn crate::http::HttpProvider,
    set_num: &str,
    limit: u64,
) -> anyhow::Result<BricksetSet> {
    let api_key = env::var("BRICKSET_API_KEY")?;
    // the set num is typically `{number}-1` but if they just give `10281` we should maybe append `-1` if there is no hyphen?
    let set_number = if set_num.contains('-') {
        set_num.to_string()
    } else {
        format!("{set_num}-1")
    };

    let params = format!("{{\"setNumber\": \"{set_number}\"}}");
    let res_text = http
        .post_form(
            "https://brickset.com/api/v3.asmx/getSets",
            vec![
                ("apiKey".to_string(), api_key),
                ("userHash".to_string(), String::new()),
                ("params".to_string(), params),
            ],
            limit,
        )
        .await?;
    let res: BricksetResponse = serde_json::from_str(&res_text)?;

    if res.status == "success" {
        if let Some(sets) = res.sets {
            if let Some(set) = sets.into_iter().next() {
                return Ok(set);
            }
        }
    }

    Err(anyhow::anyhow!("Set not found or API error"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::MockHttpProvider;

    #[tokio::test]
    async fn test_fetch_part_mock() {
        let mut mock_http = MockHttpProvider::new();
        mock_http.expect_get_json_with_auth()
            .times(1)
            .returning(|_, _, _, _| {
                Ok(r#"{"part_num": "3001", "name": "Brick 2 x 4", "part_url": "url", "year_from": 1958, "year_to": 2024, "molds": [], "alternates": [], "external_ids": {}}"#.to_string())
            });

        std::env::set_var("REBRICKABLE_API_KEY", "test");
        let result = fetch_part(&mock_http, "3001", 1024 * 1024).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name, "Brick 2 x 4");
    }

    #[tokio::test]
    async fn test_fetch_set_mock() {
        let mut mock_http = MockHttpProvider::new();
        mock_http.expect_post_form()
            .times(1)
            .returning(|_, _, _| {
                Ok(r#"{"status": "success", "sets": [{"number": "42083", "numberVariant": 1, "name": "Bugatti", "year": 2018, "theme": "Technic"}]}"#.to_string())
            });

        std::env::set_var("BRICKSET_API_KEY", "test");
        let result = fetch_set(&mock_http, "42083", 1024 * 1024).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name, "Bugatti");
    }

    #[test]
    fn test_parse_rebrickable_part() {
        let json = r#"
        {
            "part_num": "3001",
            "name": "Brick 2 x 4",
            "part_cat_id": 11,
            "part_url": "https://rebrickable.com/parts/3001/brick-2-x-4/",
            "part_img_url": "https://cdn.rebrickable.com/media/parts/elements/300126.jpg",
            "external_ids": {
                "BrickLink": ["3001"],
                "Lego": ["3001"]
            },
            "print_of": null,
            "year_from": 1958,
            "year_to": 2024,
            "molds": ["3001a", "3001b"],
            "alternates": []
        }
        "#;

        let part: RebrickablePart = serde_json::from_str(json).unwrap();
        assert_eq!(part.part_num, "3001");
        assert_eq!(part.name, "Brick 2 x 4");
        assert_eq!(part.year_from, 1958);
        assert_eq!(part.molds.len(), 2);
        assert!(part.print_of.is_none());
        assert_eq!(part.external_ids.get("BrickLink").unwrap()[0], "3001");
    }

    #[test]
    fn test_parse_brickset_response() {
        let json = r#"
        {
            "status": "success",
            "matches": 1,
            "sets": [
                {
                    "setID": 28659,
                    "number": "42083",
                    "numberVariant": 1,
                    "name": "Bugatti Chiron",
                    "year": 2018,
                    "theme": "Technic",
                    "themeGroup": "Technical",
                    "subtheme": "Ultimate Car Concept",
                    "category": "Normal",
                    "released": true,
                    "pieces": 3599,
                    "minifigs": 0,
                    "image": {
                        "thumbnailURL": "https://images.brickset.com/sets/small/42083-1.jpg",
                        "imageURL": "https://images.brickset.com/sets/images/42083-1.jpg"
                    },
                    "bricksetURL": "https://brickset.com/sets/42083-1",
                    "rating": 4.6
                }
            ]
        }
        "#;

        let res: BricksetResponse = serde_json::from_str(json).unwrap();
        assert_eq!(res.status, "success");
        let sets = res.sets.unwrap();
        assert_eq!(sets.len(), 1);
        let set = &sets[0];
        assert_eq!(set.number, "42083");
        assert_eq!(set.name, "Bugatti Chiron");
        assert_eq!(set.year, 2018);
        assert_eq!(set.pieces, Some(3599));
        assert_eq!(
            set.image.as_ref().unwrap().imageURL.as_deref(),
            Some("https://images.brickset.com/sets/images/42083-1.jpg")
        );
        assert_eq!(set.rating, Some(4.6));
    }
}
