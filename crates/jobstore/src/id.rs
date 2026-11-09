use std::fmt;

/// Opaque job identifier, also used as the job directory's name.
///
/// Matches `#/definitions/JobId` in the protocol schema
/// (`^[A-Za-z0-9_-]{1,64}$`). New IDs are `j_` followed by 16 random hex
/// digits, so they reveal nothing about the job and can't express a path.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct JobId(String);

impl JobId {
    /// A fresh random ID.
    pub fn new_random() -> Self {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).expect("the OS random number generator is unavailable");
        let mut s = String::with_capacity(18);
        s.push_str("j_");
        for b in bytes {
            s.push_str(&format!("{b:02x}"));
        }
        JobId(s)
    }

    /// Parses an ID, accepting only what the protocol schema allows.
    pub fn parse(s: &str) -> Option<Self> {
        let ok = !s.is_empty()
            && s.len() <= 64
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        ok.then(|| JobId(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JobId({})", self.0)
    }
}

impl TryFrom<String> for JobId {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        JobId::parse(&s).ok_or_else(|| "invalid job id".to_owned())
    }
}

impl From<JobId> for String {
    fn from(id: JobId) -> String {
        id.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_ids_are_valid_and_distinct() {
        let a = JobId::new_random();
        let b = JobId::new_random();
        assert_ne!(a, b);
        assert_eq!(a.as_str().len(), 18);
        assert_eq!(JobId::parse(a.as_str()), Some(a));
    }

    #[test]
    fn parse_rejects_paths_and_junk() {
        for bad in ["", "..", "a/b", "a\\b", "j 1", "é", &"x".repeat(65)] {
            assert_eq!(JobId::parse(bad), None, "{bad:?}");
        }
        assert!(JobId::parse("j_7f3a").is_some());
    }
}
