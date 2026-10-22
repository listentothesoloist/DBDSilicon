use super::*;
use serde_json::json;

#[test]
fn gamesdb_images_take_their_size_as_jpeg() {
    let cover =
        json!({"url_format": "https://images.gog.com/7fbb{formatter}.{ext}?namespace=gamesdb"});
    assert_eq!(
        image(&cover, "_glx_vertical_cover").as_deref(),
        Some("https://images.gog.com/7fbb_glx_vertical_cover.jpg?namespace=gamesdb")
    );
    assert_eq!(
        image(&cover, "").as_deref(),
        Some("https://images.gog.com/7fbb.jpg?namespace=gamesdb")
    );
    assert_eq!(image(&json!(null), ""), None);
    assert_eq!(
        image(
            &json!({"url_format": "http://images.gog.com/a{formatter}.{ext}"}),
            ""
        ),
        None
    );
}

#[test]
fn titles_prefer_every_languages_then_english() {
    assert_eq!(
        localized(&json!({"*": "The Witcher", "en-US": "The Witcher (EN)"})).as_deref(),
        Some("The Witcher")
    );
    assert_eq!(
        localized(&json!({"en-US": "Cyberpunk 2077"})).as_deref(),
        Some("Cyberpunk 2077")
    );
    assert_eq!(localized(&json!({"*": ""})), None);
}

#[test]
fn library_records_name_their_product_either_way() {
    let release =
        Release::of(&json!({"platform_id": "gog", "external_id": 1207658924, "certificate": "c"}));
    assert_eq!(release.external_id, "1207658924");
    assert_eq!(release.certificate.as_deref(), Some("c"));
    let release = Release::of(&json!({"platform_id": "steam", "external_id": "620"}));
    assert_eq!(
        (release.platform_id.as_str(), release.external_id.as_str()),
        ("steam", "620")
    );
}

#[test]
fn endpoints_escape_their_segments() {
    assert_eq!(
        endpoint(
            "https://galaxy-library.gog.com",
            &["users", "48628349957132247", "releases"]
        )
        .unwrap()
        .as_str(),
        "https://galaxy-library.gog.com/users/48628349957132247/releases"
    );
    assert_eq!(
        endpoint("https://api.gog.com", &["products", "../x"])
            .unwrap()
            .as_str(),
        "https://api.gog.com/products/..%2Fx"
    );
}

#[test]
fn a_gamesdb_release_tells_the_games_page() {
    let info = GameInfo::of_release(&json!({
        "summary": {"*": "Short."},
        "first_release_date": "2014-10-13T00:00:00+0000",
        "available_languages": [{"code": "en-US"}, {"code": "fr-FR"}],
        "game": {
            "summary": {"*": "The Evil Within embodies survival horror."},
            "first_release_date": "2014-10-14T00:00:00+0000",
            "developers": [{"name": "Tango Gameworks"}],
            "publishers": [{"name": "Bethesda Softworks"}],
            "genres": [{"name": {"*": "Adventure", "en-US": "Adventure"}}, {"name": {"*": "Shooter"}}],
            "aggregated_rating": 75.1176,
        },
    }));
    assert_eq!(
        info,
        GameInfo {
            summary: "The Evil Within embodies survival horror.".into(),
            developers: vec!["Tango Gameworks".into()],
            publishers: vec!["Bethesda Softworks".into()],
            release_date: Some(1_413_158_400),
            genres: vec!["Adventure".into(), "Shooter".into()],
            rating: Some(75),
            languages: vec!["en-US".into(), "fr-FR".into()],
        }
    );
    let unknown = GameInfo::of_release(&json!({"game": {"aggregated_rating": 0}}));
    assert_eq!(
        unknown,
        GameInfo::default(),
        "an unrated game has no rating"
    );
}

#[test]
fn a_game_joins_the_library_when_its_record_was_created_or_owned_first() {
    assert_eq!(
        added_at(&json!({"date_created": 1_590_000_000, "owned_since": 1_500_000_000})),
        Some(1_500_000_000)
    );
    assert_eq!(
        added_at(&json!({"date_created": 1_590_000_000, "owned_since": null})),
        Some(1_590_000_000)
    );
    assert_eq!(added_at(&json!({"date_created": 0})), None);
    assert_eq!(added_at(&json!({})), None);
}
