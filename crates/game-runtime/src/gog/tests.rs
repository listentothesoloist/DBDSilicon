use super::*;
use crate::supervisor::Provider as _;

fn configuration(platform: Platform, prefix: Option<&str>) -> Configuration {
    Configuration {
        home: "/Users/player".into(),
        install: "/library/Games/GOG/gog-1207658924-windows".into(),
        session_directory: "/library/GOGSessions".into(),
        helper: "/Obelisk.app/Contents/Helpers/SteamNative/obelisk-steam-host".into(),
        product_id: "1207658924".into(),
        platform,
        title: "The Witcher".into(),
        prefix: prefix.map(Into::into),
    }
}

#[test]
fn a_configuration_runs_a_mac_build_natively_and_a_windows_build_in_wine() {
    let account = "48628349957132247";
    configuration(Platform::Mac, None)
        .validate(account)
        .unwrap();
    configuration(
        Platform::Windows,
        Some("/library/Prefixes/gog-1207658924-windows"),
    )
    .validate(account)
    .unwrap();
    for refused in [
        configuration(Platform::Mac, Some("/library/Prefixes/x")),
        configuration(Platform::Windows, None),
        configuration(Platform::Windows, Some("relative")),
        Configuration {
            product_id: "../1207658924".into(),
            ..configuration(Platform::Mac, None)
        },
    ] {
        assert!(refused.validate(account).is_err(), "{refused:?}");
    }
    assert!(
        configuration(Platform::Mac, None)
            .validate("not-a-user")
            .is_err()
    );
}

#[test]
fn a_configuration_keeps_gog_s_spelling() {
    let mac = serde_json::to_value(configuration(Platform::Mac, None)).unwrap();
    assert_eq!(mac["platform"], "macos");
    let windows = configuration(
        Platform::Windows,
        Some("/library/Prefixes/gog-1207658924-windows"),
    );
    let written = serde_json::to_value(&windows).unwrap();
    assert_eq!(written["platform"], "windows");
    assert_eq!(
        serde_json::from_value::<Configuration>(written).unwrap(),
        windows
    );
}

#[test]
fn a_configuration_from_before_titles_still_loads() {
    let saved = serde_json::json!({
        "home": "/Users/player",
        "install": "/library/Games/GOG/gog-1207658924-windows",
        "session_directory": "/library/GOGSessions",
        "helper": "/Obelisk.app/Contents/Helpers/SteamNative/obelisk-steam-host",
        "product_id": "1207658924",
        "platform": "windows",
        "prefix": "/library/Prefixes/gog-1207658924-windows",
    });
    let loaded: Configuration = serde_json::from_value(saved).unwrap();
    assert_eq!(
        loaded,
        Configuration {
            title: String::new(),
            ..configuration(
                Platform::Windows,
                Some("/library/Prefixes/gog-1207658924-windows")
            )
        }
    );
}

#[test]
fn a_cancelled_gog_launch_ends_silently() {
    assert!(Gog::cancelled(&Error::Cancelled));
    assert!(Gog::cancelled(&Error::Gog(gog_kit::GogError::Cancelled)));
    assert!(!Gog::cancelled(&Error::Session));
}
