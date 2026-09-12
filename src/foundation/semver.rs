#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    pub fn within(self, low: Version, high: Version) -> bool {
        self >= low && self <= high
    }
}

pub fn coerce(input: &str) -> Option<Version> {
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if index - start > 16 {
            continue;
        }
        let major = input[start..index].parse().ok()?;
        let (minor, next) = read_component(input, index);
        let (patch, _) = match minor {
            Some(_) => read_component(input, next),
            None => (None, next),
        };
        return Some(Version::new(major, minor.unwrap_or(0), patch.unwrap_or(0)));
    }
    None
}

fn read_component(input: &str, index: usize) -> (Option<u64>, usize) {
    let bytes = input.as_bytes();
    if index >= bytes.len() || bytes[index] != b'.' {
        return (None, index);
    }
    let start = index + 1;
    let mut end = start;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    if end == start || end - start > 16 {
        return (None, index);
    }
    match input[start..end].parse() {
        Ok(value) => (Some(value), end),
        Err(_) => (None, index),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coerces_like_node_semver() {
        assert_eq!(coerce("1.20.1"), Some(Version::new(1, 20, 1)));
        assert_eq!(coerce("1.12"), Some(Version::new(1, 12, 0)));
        assert_eq!(
            coerce("2.9.4-nightly-20150209"),
            Some(Version::new(2, 9, 4))
        );
        assert_eq!(
            coerce("1.20.1-20230612.114412"),
            Some(Version::new(1, 20, 1))
        );
        assert_eq!(coerce("20150209"), Some(Version::new(20150209, 0, 0)));
        assert_eq!(coerce("v3.3.1"), Some(Version::new(3, 3, 1)));
        assert_eq!(coerce("1.2.3.4"), Some(Version::new(1, 2, 3)));
        assert_eq!(coerce("latest_release"), None);
        assert_eq!(coerce("nightly"), None);
    }

    #[test]
    fn compares_and_ranges() {
        let low = Version::new(1, 14, 4);
        let high = Version::new(1, 18, 2);
        assert!(Version::new(1, 16, 5).within(low, high));
        assert!(Version::new(1, 14, 4).within(low, high));
        assert!(!Version::new(1, 19, 0).within(low, high));
        assert!(Version::new(9, 5, 0) > Version::new(9, 1, 0));
    }
}
