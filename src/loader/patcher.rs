use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::foundation::events::{Event, EventHandler};
use crate::foundation::maven::library_path;
use crate::foundation::os::Platform;

use super::{Error, InstallConfig, InstallSection, archive, emit};

pub struct Patcher<'a> {
    config: &'a InstallConfig,
    universal_prefix: &'a str,
}

impl<'a> Patcher<'a> {
    pub fn new(config: &'a InstallConfig, universal_prefix: &'a str) -> Self {
        Self {
            config,
            universal_prefix,
        }
    }

    fn libraries_dir(&self) -> PathBuf {
        self.config.loader_dir.join("libraries")
    }

    fn library_file(&self, coordinate: &str) -> PathBuf {
        let info = library_path(coordinate, None, None);
        self.libraries_dir().join(info.path).join(info.name)
    }

    pub fn check(&self, profile: &InstallSection) -> bool {
        let mut files = Vec::new();
        for processor in &profile.processors {
            if let Some(sides) = &processor.sides
                && !sides.iter().any(|side| side == "client")
            {
                continue;
            }
            for arg in &processor.args {
                let key = strip_braces(arg);
                if let Some(entry) = profile.data.get(&key) {
                    if key == "BINPATCH" {
                        continue;
                    }
                    files.push(entry.client.clone());
                }
            }
        }
        let unique: HashSet<String> = files.into_iter().collect();
        for file in unique {
            let coordinate = file.replacen('[', "", 1).replacen(']', "", 1);
            if !self.library_file(&coordinate).exists() {
                return false;
            }
        }
        true
    }

    pub async fn patch(
        &self,
        profile: &InstallSection,
        events: &EventHandler,
    ) -> Result<(), Error> {
        for processor in &profile.processors {
            if let Some(sides) = &processor.sides
                && !sides.iter().any(|side| side == "client")
            {
                continue;
            }
            let jar_path = self.library_file(&processor.jar);
            let args: Vec<String> = processor
                .args
                .iter()
                .map(|arg| self.set_argument(arg, profile))
                .map(|arg| self.compute_path(&arg))
                .collect();
            let mut classpath = vec![jar_path.display().to_string()];
            classpath.extend(
                processor
                    .classpath
                    .iter()
                    .map(|entry| self.library_file(entry).display().to_string()),
            );

            let Some(main_class) = archive::main_class(&jar_path)? else {
                emit(
                    events,
                    Event::Error(format!(
                        "Unable to determine the main class of the jar: {}",
                        jar_path.display()
                    )),
                );
                continue;
            };

            let separator = Platform::current().classpath_separator().to_string();
            let mut command = Command::new(&self.config.java_path);
            command
                .arg("-classpath")
                .arg(classpath.join(&separator))
                .arg(&main_class)
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = command
                .spawn()
                .map_err(|source| super::io_error(&self.config.java_path, source))?;

            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            let out_events = events.clone();
            let out_task = tokio::spawn(async move {
                if let Some(stdout) = stdout {
                    let mut lines = BufReader::new(stdout).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        out_events(Event::Patch(line));
                    }
                }
            });
            let err_events = events.clone();
            let err_task = tokio::spawn(async move {
                if let Some(stderr) = stderr {
                    let mut lines = BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        err_events(Event::Patch(line));
                    }
                }
            });
            let status = child
                .wait()
                .await
                .map_err(|source| super::io_error(&self.config.java_path, source))?;
            let _ = out_task.await;
            let _ = err_task.await;
            if !status.success() {
                emit(
                    events,
                    Event::Error(format!(
                        "The {} patcher exited with {status}",
                        self.config.kind
                    )),
                );
            }
        }
        Ok(())
    }

    fn set_argument(&self, arg: &str, profile: &InstallSection) -> String {
        let key = strip_braces(arg);
        if let Some(entry) = profile.data.get(&key) {
            if key == "BINPATCH" {
                let universal = profile
                    .libraries
                    .iter()
                    .find(|library| library.name.starts_with(self.universal_prefix))
                    .map(|library| library.name.as_str());
                let coordinate = profile.path.as_deref().or(universal).unwrap_or_default();
                let info = library_path(coordinate, None, None);
                let jar = self.libraries_dir().join(info.path).join(info.name);
                return jar
                    .display()
                    .to_string()
                    .replacen(".jar", "-clientdata.lzma", 1);
            }
            return unquote(&entry.client);
        }
        let libraries = self.libraries_dir().display().to_string();
        arg.replace("{SIDE}", "client")
            .replace("{ROOT}", &self.config.loader_dir.display().to_string())
            .replace(
                "{MINECRAFT_JAR}",
                &self.config.minecraft_jar.display().to_string(),
            )
            .replace(
                "{MINECRAFT_VERSION}",
                &self.config.minecraft_json.display().to_string(),
            )
            .replace("{INSTALLER}", &libraries)
            .replace("{LIBRARY_DIR}", &libraries)
    }

    fn compute_path(&self, arg: &str) -> String {
        if arg.starts_with('[') {
            let coordinate = arg.replacen('[', "", 1).replacen(']', "", 1);
            return self.library_file(&coordinate).display().to_string();
        }
        arg.to_owned()
    }
}

fn strip_braces(arg: &str) -> String {
    arg.replacen('{', "", 1).replacen('}', "", 1)
}

fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('\'') && trimmed.ends_with('\''))
            || (trimmed.starts_with('"') && trimmed.ends_with('"')))
    {
        return trimmed[1..trimmed.len() - 1].to_owned();
    }
    value.to_owned()
}

#[allow(dead_code)]
fn exists(path: &Path) -> bool {
    path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foundation::options::LoaderKind;
    use crate::loader::{DataEntry, Processor};
    use crate::providers::launchermeta::Library;
    use std::collections::HashMap;
    use std::time::Duration;

    fn config() -> InstallConfig {
        InstallConfig {
            kind: LoaderKind::Forge,
            minecraft_version: "1.20.1".into(),
            build: "latest".into(),
            loader_dir: PathBuf::from("/root/loader"),
            java_path: PathBuf::from("/java"),
            minecraft_jar: PathBuf::from("/root/versions/1.20.1/1.20.1.jar"),
            minecraft_json: PathBuf::from("/root/versions/1.20.1/1.20.1.json"),
            concurrency: 5,
            timeout: Duration::from_secs(10),
        }
    }

    fn profile() -> InstallSection {
        let mut data = HashMap::new();
        data.insert(
            "MAPPINGS".to_owned(),
            DataEntry {
                client: "[de.oceanlabs.mcp:mcp_config:1.20.1-20230612.114412:mappings@txt]".into(),
                server: String::new(),
            },
        );
        data.insert(
            "BINPATCH".to_owned(),
            DataEntry {
                client: "/data/client.lzma".into(),
                server: String::new(),
            },
        );
        data.insert(
            "MC_SLIM_SHA".to_owned(),
            DataEntry {
                client: "'045d6edac8fe1b004e159c49704b5b729f551079'".into(),
                server: String::new(),
            },
        );
        InstallSection {
            libraries: vec![Library {
                name: "net.minecraftforge:forge:1.20.1-47.2.0:universal".into(),
                downloads: None,
                natives: None,
                rules: None,
                extract: None,
                url: None,
            }],
            processors: vec![Processor {
                jar: "net.minecraftforge:binarypatcher:1.1.1".into(),
                args: vec![
                    "--clean".into(),
                    "{MC_SRG}".into(),
                    "--apply".into(),
                    "{BINPATCH}".into(),
                ],
                classpath: vec![],
                sides: None,
            }],
            data,
            path: None,
            file_path: None,
            json: Some("/version.json".into()),
        }
    }

    #[test]
    fn resolves_arguments_like_node() {
        let config = config();
        let patcher = Patcher::new(&config, "net.minecraftforge:forge");
        let profile = profile();
        let mapping = patcher.compute_path(&patcher.set_argument("{MAPPINGS}", &profile));
        assert_eq!(
            mapping,
            "/root/loader/libraries/de/oceanlabs/mcp/mcp_config/1.20.1-20230612.114412/mcp_config-1.20.1-20230612.114412-mappings.txt"
        );
        let binpatch = patcher.set_argument("{BINPATCH}", &profile);
        assert_eq!(
            binpatch,
            "/root/loader/libraries/net/minecraftforge/forge/1.20.1-47.2.0/forge-1.20.1-47.2.0-universal-clientdata.lzma"
        );
        assert_eq!(
            patcher.set_argument("{MC_SLIM_SHA}", &profile),
            "045d6edac8fe1b004e159c49704b5b729f551079"
        );
        assert_eq!(
            patcher.set_argument("{MINECRAFT_JAR}", &profile),
            "/root/versions/1.20.1/1.20.1.jar"
        );
        assert_eq!(patcher.set_argument("{SIDE}", &profile), "client");
        assert_eq!(
            patcher.set_argument("{ROOT}/run.sh", &profile),
            "/root/loader/run.sh"
        );
        assert_eq!(patcher.set_argument("--task", &profile), "--task");
    }

    #[test]
    fn check_reports_missing_outputs() {
        let config = config();
        let patcher = Patcher::new(&config, "net.minecraftforge:forge");
        let mut profile = profile();
        profile.processors[0].args = vec![
            "--output".into(),
            "{MAPPINGS}".into(),
            "--apply".into(),
            "{BINPATCH}".into(),
        ];
        assert!(!patcher.check(&profile));
        profile.processors[0].sides = Some(vec!["server".into()]);
        assert!(patcher.check(&profile));
    }
}
