//! Which of a game's builds: the Mac build or the Windows build, whatever
//! each storefront calls it.
use serde::{Deserialize, Serializer};

/// Which of a game's builds: the Mac build, which runs natively, or the
/// Windows build, which runs in Wine. It reads every spelling an Obelisk
/// record ever held, and is written only as a storefront spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
pub enum Platform {
    #[serde(rename = "macos", alias = "Mac")]
    Mac,
    #[serde(rename = "windows", alias = "Windows")]
    Windows,
}

/// How one storefront spells the two builds in its requests, receipts,
/// folder names and ids. Each kit declares its own.
#[derive(Clone, Copy, Debug)]
pub struct Spelling {
    mac: &'static str,
    windows: &'static str,
}

impl Spelling {
    pub const fn new(mac: &'static str, windows: &'static str) -> Self {
        Self { mac, windows }
    }

    /// `platform`, as this storefront spells it.
    pub const fn name(self, platform: Platform) -> &'static str {
        match platform {
            Platform::Mac => self.mac,
            Platform::Windows => self.windows,
        }
    }

    /// Writes `platform` as this storefront spells it, for a record's
    /// `serialize_with` helper.
    pub fn serialize<S: Serializer>(
        self,
        platform: Platform,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name(platform))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPIC: Spelling = Spelling::new("Mac", "Windows");

    #[derive(serde::Serialize)]
    struct Record {
        #[serde(serialize_with = "epic")]
        platform: Platform,
    }

    fn epic<S: Serializer>(platform: &Platform, serializer: S) -> Result<S::Ok, S::Error> {
        EPIC.serialize(*platform, serializer)
    }

    #[test]
    fn every_spelling_a_record_ever_held_reads() {
        for (spelling, platform) in [
            ("macos", Platform::Mac),
            ("Mac", Platform::Mac),
            ("windows", Platform::Windows),
            ("Windows", Platform::Windows),
        ] {
            assert_eq!(
                serde_json::from_value::<Platform>(spelling.into()).unwrap(),
                platform,
                "{spelling}"
            );
        }
        for spelling in ["osx", "linux", "MACOS", ""] {
            assert!(
                serde_json::from_value::<Platform>(spelling.into()).is_err(),
                "{spelling}"
            );
        }
    }

    #[test]
    fn a_spelling_names_each_build_its_own_way() {
        assert_eq!(EPIC.name(Platform::Mac), "Mac");
        assert_eq!(EPIC.name(Platform::Windows), "Windows");
        assert_eq!(
            serde_json::to_value(Record {
                platform: Platform::Mac
            })
            .unwrap(),
            serde_json::json!({ "platform": "Mac" })
        );
    }
}
