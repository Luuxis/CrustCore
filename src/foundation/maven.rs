use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MavenCoordinate {
    pub group: String,
    pub artifact: String,
    pub version: String,
    pub classifier: Option<String>,
    pub extension: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid maven coordinate: {0}")]
pub struct ParseError(pub String);

impl FromStr for MavenCoordinate {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let (body, extension) = match input.rsplit_once('@') {
            Some((body, ext)) if !ext.is_empty() => (body, ext.to_owned()),
            _ => (input, "jar".to_owned()),
        };
        let mut parts = body.split(':');
        let (Some(group), Some(artifact), Some(version)) =
            (parts.next(), parts.next(), parts.next())
        else {
            return Err(ParseError(input.to_owned()));
        };
        if group.is_empty() || artifact.is_empty() || version.is_empty() {
            return Err(ParseError(input.to_owned()));
        }
        let classifier = parts.next().filter(|c| !c.is_empty()).map(str::to_owned);
        if parts.next().is_some() {
            return Err(ParseError(input.to_owned()));
        }
        Ok(Self {
            group: group.to_owned(),
            artifact: artifact.to_owned(),
            version: version.to_owned(),
            classifier,
            extension,
        })
    }
}

impl MavenCoordinate {
    pub fn file_name(&self) -> String {
        match &self.classifier {
            Some(classifier) => format!(
                "{}-{}-{}.{}",
                self.artifact, self.version, classifier, self.extension
            ),
            None => format!("{}-{}.{}", self.artifact, self.version, self.extension),
        }
    }

    pub fn directory(&self) -> String {
        format!(
            "{}/{}/{}",
            self.group.replace('.', "/"),
            self.artifact,
            self.version
        )
    }

    pub fn path(&self) -> String {
        format!("{}/{}", self.directory(), self.file_name())
    }

    pub fn key(&self) -> String {
        match &self.classifier {
            Some(classifier) => format!("{}:{}:{}", self.group, self.artifact, classifier),
            None => format!("{}:{}", self.group, self.artifact),
        }
    }
}

impl fmt::Display for MavenCoordinate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.group, self.artifact, self.version)?;
        if let Some(classifier) = &self.classifier {
            write!(f, ":{classifier}")?;
        }
        if self.extension != "jar" {
            write!(f, "@{}", self.extension)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryPath {
    pub path: String,
    pub name: String,
    pub version: String,
}

pub fn library_path(
    main: &str,
    native_suffix: Option<&str>,
    force_ext: Option<&str>,
) -> LibraryPath {
    let parts: Vec<&str> = main.split(':').collect();
    let group = parts.first().copied().unwrap_or_default();
    let artifact = parts.get(1).copied().unwrap_or_default();
    let version = parts.get(2).copied().unwrap_or_default();
    let file_name = match parts.get(3) {
        Some(classifier) => format!("{version}-{classifier}"),
        None => version.to_owned(),
    };
    let final_file_name = if file_name.contains('@') {
        file_name.replacen('@', ".", 1)
    } else {
        format!(
            "{file_name}{}{}",
            native_suffix.unwrap_or_default(),
            force_ext.unwrap_or(".jar")
        )
    };
    let version_dir = version.split('@').next().unwrap_or_default();
    LibraryPath {
        path: format!("{}/{artifact}/{version_dir}", group.replace('.', "/")),
        name: format!("{artifact}-{final_file_name}"),
        version: version.to_owned(),
    }
}

impl LibraryPath {
    pub fn full(&self) -> String {
        format!("{}/{}", self.path, self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_path_matches_node_get_path_libraries() {
        let forge = library_path("net.minecraftforge:forge:1.19-41.0.63", None, None);
        assert_eq!(forge.path, "net/minecraftforge/forge/1.19-41.0.63");
        assert_eq!(forge.name, "forge-1.19-41.0.63.jar");
        assert_eq!(forge.version, "1.19-41.0.63");

        let mapping = library_path(
            "de.oceanlabs.mcp:mcp_config:1.20.1-20230612.114412:mappings@txt",
            None,
            None,
        );
        assert_eq!(
            mapping.path,
            "de/oceanlabs/mcp/mcp_config/1.20.1-20230612.114412"
        );
        assert_eq!(
            mapping.name,
            "mcp_config-1.20.1-20230612.114412-mappings.txt"
        );

        let zip = library_path(
            "de.oceanlabs.mcp:mcp_config:1.12.2-20200226.224830@zip",
            None,
            None,
        );
        assert_eq!(
            zip.path,
            "de/oceanlabs/mcp/mcp_config/1.12.2-20200226.224830"
        );
        assert_eq!(zip.name, "mcp_config-1.12.2-20200226.224830.zip");
        assert_eq!(zip.version, "1.12.2-20200226.224830@zip");

        let at_jar = library_path("org.ow2.asm:asm-analysis:9.7@jar", None, None);
        assert_eq!(at_jar.name, "asm-analysis-9.7.jar");
        assert_eq!(at_jar.path, "org/ow2/asm/asm-analysis/9.7");

        let native = library_path(
            "org.lwjgl.lwjgl:lwjgl-platform:2.9.4-nightly-20150209",
            Some("-natives-osx"),
            None,
        );
        assert_eq!(
            native.name,
            "lwjgl-platform-2.9.4-nightly-20150209-natives-osx.jar"
        );

        let client_data = library_path(
            "net.minecraftforge:forge:1.20.1-47.2.0:universal",
            Some("-clientdata"),
            Some(".lzma"),
        );
        assert_eq!(
            client_data.name,
            "forge-1.20.1-47.2.0-universal-clientdata.lzma"
        );
        assert_eq!(
            client_data.full(),
            "net/minecraftforge/forge/1.20.1-47.2.0/forge-1.20.1-47.2.0-universal-clientdata.lzma"
        );
    }

    #[test]
    fn parses_simple_coordinate() {
        let c: MavenCoordinate = "com.mojang:brigadier:1.0.18".parse().unwrap();
        assert_eq!(c.path(), "com/mojang/brigadier/1.0.18/brigadier-1.0.18.jar");
        assert_eq!(c.key(), "com.mojang:brigadier");
    }

    #[test]
    fn parses_classifier_and_extension() {
        let c: MavenCoordinate = "org.lwjgl:lwjgl:3.3.1:natives-macos-arm64".parse().unwrap();
        assert_eq!(
            c.path(),
            "org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1-natives-macos-arm64.jar"
        );
        let c: MavenCoordinate = "net.minecraftforge:forge:1.20.1-47.2.0:universal@zip"
            .parse()
            .unwrap();
        assert_eq!(c.file_name(), "forge-1.20.1-47.2.0-universal.zip");
        assert_eq!(
            c.to_string(),
            "net.minecraftforge:forge:1.20.1-47.2.0:universal@zip"
        );
    }

    #[test]
    fn rejects_invalid() {
        assert!("only:two".parse::<MavenCoordinate>().is_err());
        assert!("a:b:c:d:e".parse::<MavenCoordinate>().is_err());
    }
}
