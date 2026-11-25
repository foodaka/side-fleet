//! Features that are on for some people only, while they're tried out.
//!
//! `SIDEFLEET_FEATURES` names each feature and the GitHub logins it's on for:
//! `review-desk=alice,bob;other=*`. `*` turns a feature on for everyone, the
//! developer key included. A feature that isn't listed is off. `GET /v1/me`
//! says which are on for the caller, and the web app shows or hides them.

use std::collections::BTreeMap;

/// The review desk: finished tasks laid out to check, with flags, notes and merge.
pub const REVIEW_DESK: &str = "review-desk";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Features(BTreeMap<String, Vec<String>>);

impl Features {
    pub fn parse(s: &str) -> Self {
        let mut map = BTreeMap::new();
        for part in s.split(';') {
            let Some((name, who)) = part.split_once('=') else { continue };
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            let who: Vec<String> = who.split(',').map(|w| w.trim().to_ascii_lowercase()).filter(|w| !w.is_empty()).collect();
            map.insert(name.to_string(), who);
        }
        Self(map)
    }

    /// The features on for this GitHub login; `None` is the developer key.
    pub fn for_login(&self, login: Option<&str>) -> Vec<String> {
        let login = login.map(str::to_ascii_lowercase);
        self.0
            .iter()
            .filter(|(_, who)| who.iter().any(|w| w == "*" || Some(w) == login.as_ref()))
            .map(|(name, _)| name.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_only_for_the_logins_named() {
        let f = Features::parse("review-desk=Alice, bob ; other=*;broken;=x");
        assert_eq!(f.for_login(Some("alice")), vec!["other", "review-desk"]);
        assert_eq!(f.for_login(Some("BOB")), vec!["other", "review-desk"]);
        assert_eq!(f.for_login(Some("carol")), vec!["other"]);
        assert_eq!(f.for_login(None), vec!["other"]);
    }

    #[test]
    fn nothing_set_means_nothing_on() {
        assert!(Features::parse("").for_login(Some("alice")).is_empty());
        assert!(Features::default().for_login(None).is_empty());
    }
}
