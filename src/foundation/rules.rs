use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::os::{Arch, Platform};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    #[serde(default)]
    pub action: Action,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<OsRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<HashMap<String, bool>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    #[default]
    Allow,
    Disallow,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OsRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    #[serde(
        rename = "versionRange",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub version_range: Option<VersionRange>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionRange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<String>,
}

pub fn evaluate_node(rules: &[Rule], platform: Platform) -> bool {
    for rule in rules {
        let mut matches = true;
        if let Some(os) = &rule.os {
            if let Some(name) = &os.name
                && name != platform.mojang_name()
            {
                matches = false;
            }
            if matches && let Some(range) = &os.version_range {
                if range.min.is_some() {
                    matches = platform == Platform::Windows;
                }
                if range.max.is_some() {
                    matches = platform == Platform::Windows;
                }
            }
        }
        let allow = rule.action == Action::Allow;
        if (allow && !matches) || (!allow && matches) {
            return false;
        }
    }
    true
}

pub fn skip_library_node(rules: Option<&[Rule]>, platform: Platform) -> bool {
    let Some(rules) = rules else {
        return false;
    };
    let mut should_skip = true;
    for rule in rules {
        if rule.features.is_some() {
            continue;
        }
        let os_matches = match &rule.os {
            Some(os) => os.name.as_deref() == Some(platform.mojang_name()),
            None => true,
        };
        if rule.action == Action::Allow && os_matches {
            should_skip = false;
        } else if rule.action == Action::Disallow && os_matches {
            should_skip = true;
        }
    }
    should_skip
}

#[derive(Debug, Clone)]
pub struct RuleContext {
    pub platform: Platform,
    pub arch: Arch,
    pub os_version: Option<String>,
    pub features: HashMap<String, bool>,
}

impl RuleContext {
    pub fn current() -> Self {
        Self::new(Platform::current(), Arch::current())
    }

    pub fn new(platform: Platform, arch: Arch) -> Self {
        Self {
            platform,
            arch,
            os_version: (platform == Platform::Windows).then(|| "10.0".to_owned()),
            features: HashMap::new(),
        }
    }

    pub fn with_feature(mut self, name: impl Into<String>, enabled: bool) -> Self {
        self.features.insert(name.into(), enabled);
        self
    }

    fn feature(&self, name: &str) -> bool {
        self.features.get(name).copied().unwrap_or(false)
    }
}

impl Rule {
    pub fn matches(&self, ctx: &RuleContext) -> bool {
        if let Some(os) = &self.os {
            if let Some(name) = &os.name
                && name != ctx.platform.mojang_name()
            {
                return false;
            }
            if let Some(arch) = &os.arch
                && !arch_matches(arch, ctx.arch)
            {
                return false;
            }
            if let Some(pattern) = &os.version
                && !version_matches(pattern, ctx.os_version.as_deref())
            {
                return false;
            }
        }
        if let Some(features) = &self.features {
            for (name, expected) in features {
                if ctx.feature(name) != *expected {
                    return false;
                }
            }
        }
        true
    }
}

fn version_matches(pattern: &str, os_version: Option<&str>) -> bool {
    let Some(os_version) = os_version else {
        return false;
    };
    let anchored = pattern.starts_with('^');
    let literal = pattern
        .trim_start_matches('^')
        .trim_end_matches('$')
        .replace("\\.", ".");
    if anchored {
        os_version.starts_with(&literal)
    } else {
        os_version.contains(&literal)
    }
}

fn arch_matches(rule_arch: &str, arch: Arch) -> bool {
    match rule_arch {
        "x86" => arch == Arch::X86,
        "x64" | "x86_64" => arch == Arch::X64,
        "arm64" | "aarch64" => arch == Arch::Arm64,
        "arm32" | "arm" => arch == Arch::Arm,
        _ => false,
    }
}

pub fn allowed(rules: &[Rule], ctx: &RuleContext) -> bool {
    if rules.is_empty() {
        return true;
    }
    let mut result = false;
    for rule in rules {
        if rule.matches(ctx) {
            result = rule.action == Action::Allow;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(platform: Platform, arch: Arch) -> RuleContext {
        RuleContext::new(platform, arch)
    }

    #[test]
    fn windows_version_rule_matches_windows_10_only() {
        let rules = vec![Rule {
            action: Action::Allow,
            os: Some(OsRule {
                name: Some("windows".into()),
                version: Some("^10\\.".into()),
                ..OsRule::default()
            }),
            features: None,
        }];
        assert!(allowed(&rules, &ctx(Platform::Windows, Arch::X64)));
        assert!(!allowed(&rules, &ctx(Platform::Linux, Arch::X64)));
        assert!(!allowed(&rules, &ctx(Platform::MacOs, Arch::Arm64)));
        assert!(version_matches("^10\\.", Some("10.0")));
        assert!(!version_matches("^10\\.", Some("6.1")));
        assert!(!version_matches("^10\\.", None));
    }

    fn os(name: &str) -> Option<OsRule> {
        Some(OsRule {
            name: Some(name.into()),
            ..OsRule::default()
        })
    }

    #[test]
    fn no_rules_allows() {
        assert!(allowed(&[], &ctx(Platform::Linux, Arch::X64)));
    }

    #[test]
    fn allow_then_disallow_for_os() {
        let rules = vec![
            Rule {
                action: Action::Allow,
                os: None,
                features: None,
            },
            Rule {
                action: Action::Disallow,
                os: os("osx"),
                features: None,
            },
        ];
        assert!(allowed(&rules, &ctx(Platform::Linux, Arch::X64)));
        assert!(!allowed(&rules, &ctx(Platform::MacOs, Arch::Arm64)));
    }

    #[test]
    fn allow_only_for_os() {
        let rules = vec![Rule {
            action: Action::Allow,
            os: os("windows"),
            features: None,
        }];
        assert!(allowed(&rules, &ctx(Platform::Windows, Arch::X64)));
        assert!(!allowed(&rules, &ctx(Platform::Linux, Arch::X64)));
    }

    #[test]
    fn features_must_match() {
        let mut features = HashMap::new();
        features.insert("has_custom_resolution".to_owned(), true);
        let rules = vec![Rule {
            action: Action::Allow,
            os: None,
            features: Some(features),
        }];
        assert!(!allowed(&rules, &ctx(Platform::Linux, Arch::X64)));
        let enabled = ctx(Platform::Linux, Arch::X64).with_feature("has_custom_resolution", true);
        assert!(allowed(&rules, &enabled));
    }

    #[test]
    fn arch_rule() {
        let rules = vec![Rule {
            action: Action::Allow,
            os: Some(OsRule {
                arch: Some("x86".into()),
                ..OsRule::default()
            }),
            features: None,
        }];
        assert!(allowed(&rules, &ctx(Platform::Windows, Arch::X86)));
        assert!(!allowed(&rules, &ctx(Platform::Windows, Arch::X64)));
    }

    #[test]
    fn node_evaluate_requires_every_rule_to_match() {
        let rules = vec![
            Rule {
                action: Action::Allow,
                os: os("osx"),
                features: None,
            },
            Rule {
                action: Action::Allow,
                os: os("linux"),
                features: None,
            },
        ];
        assert!(!evaluate_node(&rules, Platform::MacOs));
        assert!(evaluate_node(&rules[..1], Platform::MacOs));
        assert!(evaluate_node(&[], Platform::Windows));
        let windows_range = vec![Rule {
            action: Action::Allow,
            os: Some(OsRule {
                name: Some("windows".into()),
                version_range: Some(VersionRange {
                    min: Some("10.0".into()),
                    max: None,
                }),
                ..OsRule::default()
            }),
            features: None,
        }];
        assert!(evaluate_node(&windows_range, Platform::Windows));
        assert!(!evaluate_node(&windows_range, Platform::Linux));
    }

    #[test]
    fn node_skip_library_semantics() {
        assert!(!skip_library_node(None, Platform::MacOs));
        let allow_all_disallow_osx = vec![
            Rule {
                action: Action::Allow,
                os: None,
                features: None,
            },
            Rule {
                action: Action::Disallow,
                os: os("osx"),
                features: None,
            },
        ];
        assert!(skip_library_node(
            Some(&allow_all_disallow_osx),
            Platform::MacOs
        ));
        assert!(!skip_library_node(
            Some(&allow_all_disallow_osx),
            Platform::Linux
        ));
        let allow_windows = vec![Rule {
            action: Action::Allow,
            os: os("windows"),
            features: None,
        }];
        assert!(skip_library_node(Some(&allow_windows), Platform::MacOs));
        assert!(!skip_library_node(Some(&allow_windows), Platform::Windows));
        let mut features = HashMap::new();
        features.insert("x".to_owned(), true);
        let only_features = vec![Rule {
            action: Action::Allow,
            os: None,
            features: Some(features),
        }];
        assert!(skip_library_node(Some(&only_features), Platform::Linux));
    }
}
