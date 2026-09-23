//! The OTA manifest the firmware fetches from the update endpoint, plus the
//! version arithmetic that decides whether to install it.
//!
//! ```json
//! {
//!   "project": "w230-gear-indicator",
//!   "version": "0.3.0",
//!   "url": "https://updates.example.com/w230/w230-gear-indicator-0.3.0.bin",
//!   "sha256": "<hex of the .bin>",
//!   "size": 1234567,
//!   "min_version": "0.2.0",
//!   "notes": "Optional release notes shown in the app"
//! }
//! ```
//!
//! `scripts/release.sh` writes this file; the parser is deliberately strict
//! (missing/odd fields = no update) because a bad manifest must never start a
//! flash write.

use serde::Deserialize;

pub const PROJECT_NAME: &str = "w230-gear-indicator";

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub project: String,
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub size: u32,
    #[serde(default)]
    pub min_version: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManifestError {
    Json(String),
    WrongProject(String),
    BadVersion(String),
    InsecureUrl,
    BadSha256,
    BadSize,
}

impl core::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ManifestError::Json(e) => write!(f, "manifest is not valid JSON: {e}"),
            ManifestError::WrongProject(p) => write!(f, "manifest is for project '{p}'"),
            ManifestError::BadVersion(v) => write!(f, "manifest version '{v}' is not x.y.z"),
            ManifestError::InsecureUrl => write!(f, "firmware URL is not https"),
            ManifestError::BadSha256 => write!(f, "sha256 is not 64 hex chars"),
            ManifestError::BadSize => write!(f, "size is zero or implausible"),
        }
    }
}

/// Largest image we will ever accept (must fit an OTA slot; `partitions.csv`
/// gives each slot 0x1E0000 = 1 966 080 bytes).
pub const MAX_IMAGE_SIZE: u32 = 0x1E0000;

impl Manifest {
    pub fn parse(json: &str) -> Result<Manifest, ManifestError> {
        let m: Manifest =
            serde_json::from_str(json).map_err(|e| ManifestError::Json(e.to_string()))?;
        if m.project != PROJECT_NAME {
            return Err(ManifestError::WrongProject(m.project));
        }
        if parse_version(&m.version).is_none() {
            return Err(ManifestError::BadVersion(m.version));
        }
        if let Some(mv) = &m.min_version {
            if parse_version(mv).is_none() {
                return Err(ManifestError::BadVersion(mv.clone()));
            }
        }
        if !m.url.starts_with("https://") {
            return Err(ManifestError::InsecureUrl);
        }
        if m.sha256.len() != 64 || !m.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ManifestError::BadSha256);
        }
        if m.size == 0 || m.size > MAX_IMAGE_SIZE {
            return Err(ManifestError::BadSize);
        }
        Ok(m)
    }

    /// The expected digest as raw bytes.
    pub fn sha256_bytes(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (i, chunk) in self.sha256.as_bytes().chunks_exact(2).enumerate().take(32) {
            let hi = (chunk[0] as char).to_digit(16).unwrap_or(0) as u8;
            let lo = (chunk[1] as char).to_digit(16).unwrap_or(0) as u8;
            out[i] = (hi << 4) | lo;
        }
        out
    }
}

/// What the firmware should do with a parsed manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateDecision {
    /// Manifest version is newer than what runs now.
    Install,
    /// Running version is the same or newer (never downgrade over the air).
    UpToDate,
    /// Running version is below `min_version`: the image can't be applied
    /// directly (e.g. a partition-layout change) — needs a USB flash.
    TooOld { min_version: String },
}

/// `"1.2.3"` (an optional `v` prefix and `-pre`/`+build` suffix tolerated)
/// as a comparable tuple. Anything else is `None`.
pub fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.trim().trim_start_matches('v');
    let core = v.split(['-', '+']).next()?;
    let mut it = core.split('.');
    let maj = it.next()?.parse().ok()?;
    let min = it.next()?.parse().ok()?;
    let pat = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((maj, min, pat))
}

pub fn decide(current: &str, manifest: &Manifest) -> UpdateDecision {
    let cur = parse_version(current).unwrap_or((0, 0, 0));
    let new = parse_version(&manifest.version).unwrap_or((0, 0, 0));
    if let Some(mv) = manifest.min_version.as_deref().and_then(parse_version) {
        if cur < mv {
            return UpdateDecision::TooOld {
                min_version: manifest.min_version.clone().unwrap_or_default(),
            };
        }
    }
    if new > cur {
        UpdateDecision::Install
    } else {
        UpdateDecision::UpToDate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{
        "project": "w230-gear-indicator",
        "version": "0.3.0",
        "url": "https://updates.example.com/w230/w230-gear-indicator-0.3.0.bin",
        "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "size": 1200000,
        "min_version": "0.2.0",
        "notes": "Faster shift detection"
    }"#;

    #[test]
    fn parses_a_good_manifest() {
        let m = Manifest::parse(GOOD).unwrap();
        assert_eq!(m.version, "0.3.0");
        assert_eq!(m.size, 1_200_000);
        assert_eq!(m.notes.as_deref(), Some("Faster shift detection"));
        assert_eq!(m.sha256_bytes()[0], 0x01);
        assert_eq!(m.sha256_bytes()[7], 0xef);
    }

    #[test]
    fn optional_fields_may_be_absent() {
        let j = r#"{"project":"w230-gear-indicator","version":"1.0.0","url":"https://a/b.bin",
                    "sha256":"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff","size":10}"#;
        let m = Manifest::parse(j).unwrap();
        assert_eq!(m.min_version, None);
        assert_eq!(m.notes, None);
    }

    #[test]
    fn rejects_bad_manifests() {
        assert!(matches!(
            Manifest::parse("not json"),
            Err(ManifestError::Json(_))
        ));
        let other = GOOD.replace("w230-gear-indicator", "other-thing");
        assert!(matches!(
            Manifest::parse(&other),
            Err(ManifestError::WrongProject(_))
        ));
        let http = GOOD.replace("https://", "http://");
        assert_eq!(Manifest::parse(&http), Err(ManifestError::InsecureUrl));
        let badv = GOOD.replace("\"0.3.0\"", "\"three\"");
        assert!(matches!(
            Manifest::parse(&badv),
            Err(ManifestError::BadVersion(_))
        ));
        let badsha = GOOD.replace("0123456789abcdef", "0123456789abcdeg");
        assert_eq!(Manifest::parse(&badsha), Err(ManifestError::BadSha256));
        let huge = GOOD.replace("1200000", "9000000");
        assert_eq!(Manifest::parse(&huge), Err(ManifestError::BadSize));
        let zero = GOOD.replace("1200000", "0");
        assert_eq!(Manifest::parse(&zero), Err(ManifestError::BadSize));
    }

    #[test]
    fn version_parsing() {
        assert_eq!(parse_version("0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_version("v1.10.3"), Some((1, 10, 3)));
        assert_eq!(parse_version("1.2.3-rc1+build7"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version("a.b.c"), None);
    }

    #[test]
    fn decisions() {
        let m = Manifest::parse(GOOD).unwrap();
        assert_eq!(decide("0.2.0", &m), UpdateDecision::Install);
        assert_eq!(decide("0.2.9", &m), UpdateDecision::Install);
        assert_eq!(decide("0.3.0", &m), UpdateDecision::UpToDate);
        assert_eq!(decide("0.4.0", &m), UpdateDecision::UpToDate); // never downgrade
        assert_eq!(
            decide("0.1.0", &m),
            UpdateDecision::TooOld {
                min_version: "0.2.0".into()
            }
        );
        let no_min = Manifest {
            min_version: None,
            ..m.clone()
        };
        assert_eq!(decide("0.0.1", &no_min), UpdateDecision::Install);
    }
}
