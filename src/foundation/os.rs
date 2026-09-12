use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    MacOs,
    Linux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X64,
    X86,
    Arm64,
    Arm,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }

    pub fn mojang_name(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::MacOs => "osx",
            Self::Linux => "linux",
        }
    }

    pub fn node_name(self) -> &'static str {
        match self {
            Self::Windows => "win32",
            Self::MacOs => "darwin",
            Self::Linux => "linux",
        }
    }

    pub fn classpath_separator(self) -> char {
        match self {
            Self::Windows => ';',
            _ => ':',
        }
    }

    pub fn java_executable(self) -> &'static str {
        match self {
            Self::Windows => "bin/javaw.exe",
            _ => "bin/java",
        }
    }
}

impl Arch {
    pub fn current() -> Self {
        if cfg!(target_arch = "x86_64") {
            Self::X64
        } else if cfg!(target_arch = "x86") {
            Self::X86
        } else if cfg!(target_arch = "aarch64") {
            Self::Arm64
        } else {
            Self::Arm
        }
    }

    pub fn bits(self) -> &'static str {
        match self {
            Self::X64 | Self::Arm64 => "64",
            Self::X86 | Self::Arm => "32",
        }
    }

    pub fn mojang_name(self) -> &'static str {
        match self {
            Self::X64 => "x86_64",
            Self::X86 => "x86",
            Self::Arm64 => "arm64",
            Self::Arm => "arm32",
        }
    }
}

pub fn effective_arch(platform: Platform, arch: Arch, intel_enabled_mac: bool) -> Arch {
    if intel_enabled_mac && platform == Platform::MacOs && arch == Arch::Arm64 {
        Arch::X64
    } else {
        arch
    }
}

pub fn native_classifier(natives: &HashMap<String, String>, platform: Platform) -> Option<&str> {
    [platform.mojang_name(), platform.node_name()]
        .into_iter()
        .filter_map(|key| natives.get(key))
        .map(String::as_str)
        .find(|value| !value.is_empty())
}

pub fn java_runtime_platform(platform: Platform, arch: Arch) -> Option<&'static str> {
    match (platform, arch) {
        (Platform::Windows, Arch::X64) => Some("windows-x64"),
        (Platform::Windows, Arch::X86) => Some("windows-x86"),
        (Platform::Windows, Arch::Arm64) => Some("windows-arm64"),
        (Platform::MacOs, Arch::X64) => Some("mac-os"),
        (Platform::MacOs, Arch::Arm64) => Some("mac-os-arm64"),
        (Platform::Linux, Arch::X64) => Some("linux"),
        (Platform::Linux, Arch::X86) => Some("linux-i386"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_java_runtime_platforms() {
        assert_eq!(
            java_runtime_platform(Platform::MacOs, Arch::Arm64),
            Some("mac-os-arm64")
        );
        assert_eq!(
            java_runtime_platform(Platform::Windows, Arch::X64),
            Some("windows-x64")
        );
        assert_eq!(java_runtime_platform(Platform::Linux, Arch::Arm64), None);
    }

    #[test]
    fn intel_mode_only_affects_apple_silicon() {
        assert_eq!(
            effective_arch(Platform::MacOs, Arch::Arm64, true),
            Arch::X64
        );
        assert_eq!(
            effective_arch(Platform::MacOs, Arch::Arm64, false),
            Arch::Arm64
        );
        assert_eq!(
            effective_arch(Platform::Linux, Arch::Arm64, true),
            Arch::Arm64
        );
        assert_eq!(effective_arch(Platform::MacOs, Arch::X64, true), Arch::X64);
    }

    #[test]
    fn native_classifier_falls_back_to_node_platform_names() {
        let mut natives = HashMap::new();
        natives.insert("darwin".to_owned(), "natives-macos".to_owned());
        natives.insert("windows".to_owned(), String::new());
        natives.insert("win32".to_owned(), "natives-windows".to_owned());
        assert_eq!(
            native_classifier(&natives, Platform::MacOs),
            Some("natives-macos")
        );
        assert_eq!(
            native_classifier(&natives, Platform::Windows),
            Some("natives-windows")
        );
        assert_eq!(native_classifier(&natives, Platform::Linux), None);
    }

    #[test]
    fn current_platform_is_consistent() {
        let platform = Platform::current();
        assert!(!platform.mojang_name().is_empty());
        assert!(!Arch::current().bits().is_empty());
    }
}
