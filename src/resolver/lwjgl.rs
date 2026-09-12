use crate::foundation::os::Arch;

const AARCH64: &[(&str, &str)] = &[
    (
        "2.9.4",
        include_str!("../../assets/lwjgl/aarch64/2.9.4.json"),
    ),
    (
        "3.1.2",
        include_str!("../../assets/lwjgl/aarch64/3.1.2.json"),
    ),
    (
        "3.1.6",
        include_str!("../../assets/lwjgl/aarch64/3.1.6.json"),
    ),
    (
        "3.2.1",
        include_str!("../../assets/lwjgl/aarch64/3.2.1.json"),
    ),
    (
        "3.2.2",
        include_str!("../../assets/lwjgl/aarch64/3.2.2.json"),
    ),
    (
        "3.3.1",
        include_str!("../../assets/lwjgl/aarch64/3.3.1.json"),
    ),
    (
        "3.3.2",
        include_str!("../../assets/lwjgl/aarch64/3.3.2.json"),
    ),
];

const AARCH: &[(&str, &str)] = &[
    ("2.9.4", include_str!("../../assets/lwjgl/aarch/2.9.4.json")),
    ("3.1.2", include_str!("../../assets/lwjgl/aarch/3.1.2.json")),
    ("3.1.6", include_str!("../../assets/lwjgl/aarch/3.1.6.json")),
    ("3.2.1", include_str!("../../assets/lwjgl/aarch/3.2.1.json")),
    ("3.2.2", include_str!("../../assets/lwjgl/aarch/3.2.2.json")),
    ("3.3.1", include_str!("../../assets/lwjgl/aarch/3.3.1.json")),
    ("3.3.2", include_str!("../../assets/lwjgl/aarch/3.3.2.json")),
];

pub fn process(raw: &mut serde_json::Value, arch: Arch) -> bool {
    let table = match arch {
        Arch::Arm64 => AARCH64,
        Arch::Arm => AARCH,
        _ => return false,
    };
    let Some(libraries) = raw.get("libraries").and_then(|l| l.as_array()).cloned() else {
        return false;
    };
    let name_of = |library: &serde_json::Value| -> String {
        library
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or_default()
            .to_owned()
    };
    let last_part = |name: &str| name.rsplit(':').next().unwrap_or_default().to_owned();

    let jinput = libraries
        .iter()
        .map(&name_of)
        .find(|n| {
            n.starts_with("net.java.jinput:jinput-platform:")
                || n.starts_with("net.java.jinput:jinput:")
        })
        .map(|n| last_part(&n));
    let lwjgl = libraries
        .iter()
        .map(&name_of)
        .find(|n| n.starts_with("org.lwjgl:lwjgl:") || n.starts_with("org.lwjgl.lwjgl:lwjgl:"))
        .map(|n| last_part(&n));

    let mut result = libraries;
    let mut changed = false;
    if jinput.is_some() {
        result.retain(|l| !name_of(l).contains("jinput"));
        changed = true;
    }
    if let Some(version) = lwjgl {
        let key = if version.contains("2.9") {
            "2.9.4".to_owned()
        } else {
            version
        };
        let Some((_, content)) = table.iter().find(|(v, _)| *v == key) else {
            return changed && {
                raw["libraries"] = serde_json::Value::Array(result);
                true
            };
        };
        result.retain(|l| !name_of(l).contains("lwjgl"));
        if let Ok(extra) = serde_json::from_str::<serde_json::Value>(content)
            && let Some(extra_libraries) = extra.get("libraries").and_then(|l| l.as_array())
        {
            result.extend(extra_libraries.iter().cloned());
        }
        changed = true;
    }
    if changed {
        raw["libraries"] = serde_json::Value::Array(result);
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_lwjgl_and_jinput_for_arm64() {
        let mut raw = serde_json::json!({
            "id": "1.12.2",
            "libraries": [
                {"name": "org.lwjgl.lwjgl:lwjgl:2.9.4-nightly-20150209"},
                {"name": "org.lwjgl.lwjgl:lwjgl-platform:2.9.4-nightly-20150209"},
                {"name": "net.java.jinput:jinput:2.0.5"},
                {"name": "com.mojang:patchy:1.1"}
            ]
        });
        assert!(process(&mut raw, Arch::Arm64));
        let names: Vec<String> = raw["libraries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["name"].as_str().unwrap().to_owned())
            .collect();
        assert!(names.contains(&"com.mojang:patchy:1.1".to_owned()));
        assert!(names.iter().any(|n| n.contains("lwjgl")));
        assert!(names.iter().any(|n| n.contains("jinput")));
        let arm_lwjgl = raw["libraries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|l| l["name"].as_str().unwrap_or_default().contains("lwjgl"))
            .any(|l| {
                l["downloads"]["classifiers"]["natives-linux"]["url"]
                    .as_str()
                    .is_some_and(|u| u.contains("theofficialgman"))
            });
        assert!(arm_lwjgl);
    }

    #[test]
    fn leaves_other_architectures_untouched() {
        let mut raw = serde_json::json!({"libraries": [{"name": "org.lwjgl:lwjgl:3.3.1"}]});
        assert!(!process(&mut raw, Arch::X64));
    }

    #[test]
    fn embedded_tables_are_valid_json() {
        for (_, content) in AARCH64.iter().chain(AARCH.iter()) {
            let value: serde_json::Value = serde_json::from_str(content).unwrap();
            assert!(value["libraries"].is_array());
        }
    }
}
