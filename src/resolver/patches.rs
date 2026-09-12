use crate::foundation::maven::MavenCoordinate;
use crate::foundation::os::Platform;
use crate::providers::launchermeta::{Artifact, Library, LibraryDownloads};

const JNA_MIN_VERSION: [u64; 3] = [5, 13, 0];

struct Replacement {
    name: &'static str,
    path: &'static str,
    sha1: &'static str,
    size: u64,
    url: &'static str,
}

const JNA: Replacement = Replacement {
    name: "net.java.dev.jna:jna:5.13.0",
    path: "net/java/dev/jna/jna/5.13.0/jna-5.13.0.jar",
    sha1: "1200e7ebeedbe0d10062093f32925a912020e747",
    size: 1879325,
    url: "https://libraries.minecraft.net/net/java/dev/jna/jna/5.13.0/jna-5.13.0.jar",
};

const JNA_PLATFORM: Replacement = Replacement {
    name: "net.java.dev.jna:jna-platform:5.13.0",
    path: "net/java/dev/jna/jna-platform/5.13.0/jna-platform-5.13.0.jar",
    sha1: "88e9a306715e9379f3122415ef4ae759a352640d",
    size: 1363209,
    url: "https://libraries.minecraft.net/net/java/dev/jna/jna-platform/5.13.0/jna-platform-5.13.0.jar",
};

pub fn patch_argument(argument: &str, platform: Platform) -> String {
    if platform != Platform::MacOs {
        return argument.to_owned();
    }
    let mut result = argument.to_owned();
    for (prefix, replacement) in [
        ("jna-platform-", "jna-platform-5.13.0.jar"),
        ("jna-", "jna-5.13.0.jar"),
    ] {
        let mut search_from = 0;
        while let Some(found) = result[search_from..].find(prefix) {
            let start = search_from + found;
            let version_start = start + prefix.len();
            let version_end = result[version_start..]
                .find(".jar")
                .map(|offset| version_start + offset);
            let Some(version_end) = version_end else {
                break;
            };
            let version = &result[version_start..version_end];
            let is_version =
                !version.is_empty() && version.chars().all(|c| c.is_ascii_digit() || c == '.');
            let token_end = version_end + ".jar".len();
            let bounded_start = start == 0
                || !result[..start]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            if is_version && bounded_start && version_below(version, JNA_MIN_VERSION) {
                result.replace_range(start..token_end, replacement);
                search_from = start + replacement.len();
            } else {
                search_from = token_end.min(result.len());
            }
        }
    }
    result
}

pub fn apply(libraries: &[Library], platform: Platform) -> Vec<Library> {
    libraries
        .iter()
        .map(|library| patch(library, platform))
        .collect()
}

fn patch(library: &Library, platform: Platform) -> Library {
    if platform != Platform::MacOs {
        return library.clone();
    }
    let Ok(coordinate) = library.name.parse::<MavenCoordinate>() else {
        return library.clone();
    };
    if coordinate.group != "net.java.dev.jna" || coordinate.classifier.is_some() {
        return library.clone();
    }
    let replacement = match coordinate.artifact.as_str() {
        "jna" => JNA,
        "jna-platform" => JNA_PLATFORM,
        _ => return library.clone(),
    };
    if !version_below(&coordinate.version, JNA_MIN_VERSION) {
        return library.clone();
    }
    Library {
        name: replacement.name.to_owned(),
        downloads: Some(LibraryDownloads {
            artifact: Some(Artifact {
                path: Some(replacement.path.to_owned()),
                sha1: replacement.sha1.to_owned(),
                size: replacement.size,
                url: replacement.url.to_owned(),
            }),
            classifiers: None,
        }),
        natives: None,
        rules: library.rules.clone(),
        extract: None,
        url: None,
    }
}

fn version_below(version: &str, minimum: [u64; 3]) -> bool {
    let mut parts = version.split('.').map(|part| {
        part.chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse::<u64>()
            .unwrap_or(0)
    });
    let parsed = [
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    ];
    parsed < minimum
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library(name: &str) -> Library {
        Library {
            name: name.to_owned(),
            downloads: Some(LibraryDownloads {
                artifact: Some(Artifact {
                    path: Some("old".into()),
                    sha1: "old".into(),
                    size: 1,
                    url: "old".into(),
                }),
                classifiers: None,
            }),
            natives: None,
            rules: None,
            extract: None,
            url: None,
        }
    }

    #[test]
    fn upgrades_old_jna_on_macos() {
        let patched = apply(
            &[
                library("net.java.dev.jna:jna:5.8.0"),
                library("net.java.dev.jna:jna-platform:5.12.1"),
                library("com.mojang:text2speech:1.11.3"),
            ],
            Platform::MacOs,
        );
        assert_eq!(patched[0].name, "net.java.dev.jna:jna:5.13.0");
        assert_eq!(patched[1].name, "net.java.dev.jna:jna-platform:5.13.0");
        assert_eq!(patched[2].name, "com.mojang:text2speech:1.11.3");
        let artifact = patched[0]
            .downloads
            .as_ref()
            .unwrap()
            .artifact
            .as_ref()
            .unwrap();
        assert_eq!(artifact.sha1, JNA.sha1);
    }

    #[test]
    fn keeps_recent_jna_and_other_platforms() {
        let recent = apply(&[library("net.java.dev.jna:jna:5.13.0")], Platform::MacOs);
        assert_eq!(recent[0].name, "net.java.dev.jna:jna:5.13.0");
        assert_eq!(
            recent[0]
                .downloads
                .as_ref()
                .unwrap()
                .artifact
                .as_ref()
                .unwrap()
                .sha1,
            "old"
        );
        let newer = apply(&[library("net.java.dev.jna:jna:5.17.0")], Platform::MacOs);
        assert_eq!(newer[0].name, "net.java.dev.jna:jna:5.17.0");
        let windows = apply(&[library("net.java.dev.jna:jna:5.8.0")], Platform::Windows);
        assert_eq!(windows[0].name, "net.java.dev.jna:jna:5.8.0");
    }

    #[test]
    fn rewrites_merge_modules_jar_names_on_macos() {
        let arg = "-DmergeModules=jna-5.10.0.jar,jna-platform-5.10.0.jar";
        assert_eq!(
            patch_argument(arg, Platform::MacOs),
            "-DmergeModules=jna-5.13.0.jar,jna-platform-5.13.0.jar"
        );
        assert_eq!(patch_argument(arg, Platform::Windows), arg);
        let recent = "-DmergeModules=jna-5.14.0.jar,jna-platform-5.14.0.jar";
        assert_eq!(patch_argument(recent, Platform::MacOs), recent);
        let ignore = "-DignoreList=bootstraplauncher,securejarhandler,1.20.1.jar";
        assert_eq!(patch_argument(ignore, Platform::MacOs), ignore);
    }

    #[test]
    fn compares_versions() {
        assert!(version_below("5.8.0", JNA_MIN_VERSION));
        assert!(version_below("5.12.1", JNA_MIN_VERSION));
        assert!(!version_below("5.13.0", JNA_MIN_VERSION));
        assert!(!version_below("5.17.0", JNA_MIN_VERSION));
        assert!(version_below("4.4.0", JNA_MIN_VERSION));
    }
}
