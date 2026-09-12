use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::Error;
use crate::authenticator::Account;
use crate::foundation::maven::library_path;
use crate::foundation::options::LaunchOptions;
use crate::foundation::os::Platform;
use crate::foundation::rules::evaluate_node;
use crate::foundation::semver::{self, Version};
use crate::loader::LoaderJson;
use crate::providers::launchermeta::{Argument, Library, VersionJson};
use crate::resolver::{is_old, patches};

#[derive(Debug, Clone)]
pub struct ArgumentsInput<'a> {
    pub version: &'a VersionJson,
    pub loader: Option<&'a LoaderJson>,
    pub account: &'a Account,
    pub options: &'a LaunchOptions,
    pub natives_list: bool,
}

#[derive(Debug, Clone)]
pub struct LaunchPlan {
    pub java: PathBuf,
    pub args: Vec<String>,
    pub working_dir: PathBuf,
    pub main_class: String,
}

impl LaunchPlan {
    pub fn redacted_command(&self, secrets: &[&str]) -> String {
        let mut command = format!("{} {}", self.java.display(), self.args.join(" "));
        for secret in secrets.iter().filter(|s| !s.is_empty()) {
            command = command.replace(secret, "????????");
        }
        command
    }
}

enum Token {
    Text(String),
    Object,
}

pub fn build(input: &ArgumentsInput<'_>, java: PathBuf) -> Result<LaunchPlan, Error> {
    let game = game_arguments(input);
    let jvm = jvm_arguments(input);
    let (classpath, main_class) = classpath(input);
    let (loader_jvm, loader_game) = loader_arguments(input);
    let main_class = main_class.ok_or(Error::NoMainClass)?;

    let mut args = Vec::new();
    args.extend(jvm);
    args.extend(classpath);
    args.extend(loader_jvm);
    args.push(main_class.clone());
    args.extend(game);
    args.extend(loader_game);

    Ok(LaunchPlan {
        java,
        args,
        working_dir: input.options.game_dir(),
        main_class,
    })
}

pub fn game_arguments(input: &ArgumentsInput<'_>) -> Vec<String> {
    let version = input.version;
    let options = input.options;
    let account = input.account;

    let mut game: Vec<Token> = match &version.minecraft_arguments {
        Some(arguments) => arguments
            .split(' ')
            .map(|s| Token::Text(s.to_owned()))
            .collect(),
        None => version
            .arguments
            .as_ref()
            .map(|a| {
                a.game
                    .iter()
                    .map(|argument| match argument {
                        Argument::Plain(value) => Token::Text(value.clone()),
                        Argument::Conditional { .. } => Token::Object,
                    })
                    .collect()
            })
            .unwrap_or_default(),
    };

    if let Some(loader) = input.loader {
        if let Some(arguments) = &loader.minecraft_arguments {
            game.extend(arguments.split(' ').map(|s| Token::Text(s.to_owned())));
        }
        let mut seen: Vec<String> = Vec::new();
        game.retain(|token| match token {
            Token::Text(value) => {
                if seen.contains(value) {
                    false
                } else {
                    seen.push(value.clone());
                    true
                }
            }
            Token::Object => true,
        });
    }

    let user_type = if version.id.starts_with("1.16") {
        "Xbox".to_owned()
    } else {
        account.meta.kind.user_type().to_owned()
    };
    let assets_index_name = version
        .asset_index
        .as_ref()
        .map(|a| a.id.clone())
        .or_else(|| version.assets.clone())
        .unwrap_or_else(|| version.id.clone());
    let root = display(&options.root);
    let assets_root = if is_old(version) {
        format!("{root}/resources")
    } else {
        format!("{root}/assets")
    };
    let version_name = match input.loader {
        Some(loader) => loader
            .id
            .clone()
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| version.id.clone()),
        None => version.id.clone(),
    };
    let xuid = account
        .xuid()
        .map(str::to_owned)
        .unwrap_or_else(|| account.access_token.clone());
    let client_id = account
        .client_id
        .clone()
        .filter(|id| !id.is_empty())
        .or_else(|| Some(account.client_token.clone()).filter(|token| !token.is_empty()))
        .unwrap_or_else(|| account.access_token.clone());

    let mut placeholders: HashMap<&str, String> = HashMap::new();
    placeholders.insert("${auth_access_token}", account.access_token.clone());
    placeholders.insert("${auth_session}", account.access_token.clone());
    placeholders.insert("${auth_player_name}", account.name.clone());
    placeholders.insert("${auth_uuid}", account.uuid.clone());
    placeholders.insert("${auth_xuid}", xuid);
    placeholders.insert("${user_properties}", account.user_properties.clone());
    placeholders.insert("${user_type}", user_type);
    placeholders.insert("${version_name}", version_name);
    placeholders.insert("${assets_index_name}", assets_index_name);
    placeholders.insert("${game_directory}", display(&options.game_dir()));
    placeholders.insert("${assets_root}", assets_root.clone());
    placeholders.insert("${game_assets}", assets_root);
    placeholders.insert("${version_type}", version.kind.clone());
    placeholders.insert("${clientid}", client_id);

    let mut result: Vec<String> = game
        .into_iter()
        .filter_map(|token| match token {
            Token::Text(value) => Some(placeholders.get(value.as_str()).cloned().unwrap_or(value)),
            Token::Object => None,
        })
        .collect();

    if let (Some(width), Some(height)) = (options.screen.width, options.screen.height)
        && width != 0
        && height != 0
    {
        result.push("--width".to_owned());
        result.push(width.to_string());
        result.push("--height".to_owned());
        result.push(height.to_string());
    }
    if options.screen.fullscreen {
        result.push("--fullscreen".to_owned());
    }
    result.extend(options.game_args.iter().cloned());
    result
}

pub fn jvm_arguments(input: &ArgumentsInput<'_>) -> Vec<String> {
    let version = input.version;
    let options = input.options;
    let platform = Platform::current();
    let root = display(&options.root);
    let natives = format!("{root}/versions/{}/natives", version.id);

    let mut jvm = vec![
        format!("-Xms{}", options.memory.min),
        format!("-Xmx{}", options.memory.max),
        "-XX:+UnlockExperimentalVMOptions".to_owned(),
        "-XX:G1NewSizePercent=20".to_owned(),
        "-XX:G1ReservePercent=20".to_owned(),
        "-XX:MaxGCPauseMillis=50".to_owned(),
        "-XX:G1HeapRegionSize=32M".to_owned(),
        "-Dfml.ignoreInvalidMinecraftCertificates=true".to_owned(),
        format!("-Djna.tmpdir={natives}"),
        format!("-Dorg.lwjgl.system.SharedLibraryExtractPath={natives}"),
        format!("-Dio.netty.native.workdir={natives}"),
    ];

    if version.minecraft_arguments.is_none() {
        let os_specific = match platform {
            Platform::Windows => {
                "-XX:HeapDumpPath=MojangTricksIntelDriversForPerformance_javaw.exe_minecraft.exe.heapdump"
            }
            Platform::MacOs => "-XstartOnFirstThread",
            Platform::Linux => "-Xss1M",
        };
        jvm.push(os_specific.to_owned());
    }

    if options.bypass_offline {
        jvm.push("-Dminecraft.api.auth.host=https://nope.invalid/".to_owned());
        jvm.push("-Dminecraft.api.account.host=https://nope.invalid/".to_owned());
        jvm.push("-Dminecraft.api.session.host=https://nope.invalid/".to_owned());
        jvm.push("-Dminecraft.api.services.host=https://nope.invalid/".to_owned());
    }

    if input.natives_list {
        jvm.push(format!("-Djava.library.path={natives}"));
    }

    if platform == Platform::MacOs
        && let Some(assets) = &version.assets
        && let Some(icon) = dock_icon_hash(&options.root, assets)
    {
        jvm.push("-Xdock:name=Minecraft".to_owned());
        jvm.push(format!(
            "-Xdock:icon={root}/assets/objects/{}/{icon}",
            &icon[..2]
        ));
    }

    if let Some(client) = version.logging.as_ref().and_then(|l| l.client.as_ref())
        && !options.ignore_log4j
    {
        let config = format!("{root}/assets/log_configs/{}", client.file.id);
        jvm.push(client.argument.replace("${path}", &config));
    }

    let defaults = default_user_jvm_arguments(version, &jvm, &options.jvm_args, platform);
    jvm.extend(defaults);
    jvm.extend(options.jvm_args.iter().cloned());
    jvm
}

fn dock_icon_hash(root: &Path, assets: &str) -> Option<String> {
    let path = root
        .join("assets")
        .join("indexes")
        .join(format!("{assets}.json"));
    let content = std::fs::read_to_string(path).ok()?;
    let index: serde_json::Value = serde_json::from_str(&content).ok()?;
    index["objects"]["icons/minecraft.icns"]["hash"]
        .as_str()
        .map(str::to_owned)
}

fn default_user_jvm_arguments(
    version: &VersionJson,
    existing: &[String],
    user_args: &[String],
    platform: Platform,
) -> Vec<String> {
    let Some(arguments) = &version.arguments else {
        return Vec::new();
    };
    let mut keys: std::collections::HashSet<String> = existing
        .iter()
        .chain(user_args.iter())
        .map(|arg| arg_key(arg))
        .collect();
    let mut defaults = Vec::new();
    for entry in &arguments.default_user_jvm {
        if let Some(rules) = &entry.rules
            && !evaluate_node(rules, platform)
        {
            continue;
        }
        let values = entry.value.as_ref().map(|v| v.values()).unwrap_or_default();
        for value in values {
            let key = arg_key(&value);
            if !keys.contains(&key) {
                defaults.push(value);
                keys.insert(key);
            }
        }
    }
    defaults
}

pub fn arg_key(arg: &str) -> String {
    if arg.contains('=') {
        return arg.split('=').next().unwrap_or_default().to_owned();
    }
    if arg.starts_with("-XX:+") || arg.starts_with("-XX:-") {
        return arg.to_owned();
    }
    let (body, _unit) = match arg.chars().last() {
        Some(last) if "GMKgmkBb".contains(last) => (&arg[..arg.len() - last.len_utf8()], true),
        _ => (arg, false),
    };
    let digits_start = body.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    if digits_start < body.len() && digits_start >= 1 {
        return body[..digits_start].to_owned();
    }
    arg.to_owned()
}

pub fn classpath(input: &ArgumentsInput<'_>) -> (Vec<String>, Option<String>) {
    let version = input.version;
    let options = input.options;
    let platform = Platform::current();
    let os_name = platform.mojang_name();

    let mut combined: Vec<(&Library, bool)> = Vec::new();
    if let Some(loader) = input.loader {
        combined.extend(loader.libraries.iter().map(|l| (l, true)));
    }
    combined.extend(version.libraries.iter().map(|l| (l, false)));

    let current_version = semver::coerce(&options.version);
    let supported = current_version
        .map(|v| v.within(Version::new(1, 14, 4), Version::new(1, 18, 2)))
        .unwrap_or(false);
    let is_windows = platform == Platform::Windows;

    let mut order: Vec<String> = Vec::new();
    let mut latest: HashMap<String, (&Library, bool, Version)> = HashMap::new();
    for (library, is_loader) in combined {
        let parts = library_path(&library.name, None, None);
        let Some(lib_version) = semver::coerce(&parts.version) else {
            continue;
        };
        let base_path = parts
            .path
            .rsplit_once('/')
            .map(|(base, _)| base.to_owned())
            .unwrap_or_default();
        let key = format!(
            "{base_path}/{}",
            parts.name.replacen(&format!("-{}", parts.version), "", 1)
        );
        match latest.get(&key) {
            None => {
                order.push(key.clone());
                latest.insert(key, (library, is_loader, lib_version));
            }
            Some((_, _, current)) => {
                if lib_version > *current && supported && is_windows {
                    latest.insert(key, (library, is_loader, lib_version));
                }
            }
        }
    }

    let mut entries: Vec<String> = Vec::new();
    for key in &order {
        let Some((library, is_loader, _)) = latest.get(key) else {
            continue;
        };
        if *is_loader
            && library
                .name
                .starts_with("org.apache.logging.log4j:log4j-slf4j2-impl")
        {
            continue;
        }
        if let Some(natives) = &library.natives {
            if natives.get(os_name).is_none() {
                continue;
            }
        } else if let Some(rules) = &library.rules
            && let Some(first) = rules.first()
            && let Some(os) = &first.os
            && os.name.as_deref() != Some(os_name)
        {
            continue;
        }
        let info = library_path(&library.name, None, None);
        let base = if *is_loader {
            match input.loader {
                Some(loader) => display(&loader.loader_dir),
                None => display(&options.root),
            }
        } else {
            display(&options.root)
        };
        entries.push(format!("{base}/libraries/{}/{}", info.path, info.name));
    }

    match options.mcp_path() {
        Some(mcp) => entries.push(display(&mcp)),
        None => entries.push(format!(
            "{}/versions/{}/{}.jar",
            display(&options.root),
            version.id,
            version.id
        )),
    }

    let mut unique: Vec<String> = Vec::new();
    for entry in entries {
        let file_name = entry.rsplit('/').next().unwrap_or_default().to_owned();
        if !file_name.is_empty() && !unique.contains(&file_name) {
            unique.push(entry);
        }
    }

    let separator = platform.classpath_separator().to_string();
    let joined = unique.join(&separator);
    let main_class = match input.loader {
        Some(loader) => loader.main_class.clone(),
        None => Some(version.main_class.clone()),
    }
    .filter(|m| !m.is_empty());
    (vec!["-cp".to_owned(), joined], main_class)
}

pub fn loader_arguments(input: &ArgumentsInput<'_>) -> (Vec<String>, Vec<String>) {
    let Some(loader) = input.loader else {
        return (Vec::new(), Vec::new());
    };
    let separator = Platform::current().classpath_separator().to_string();
    let library_directory = format!("{}/libraries", display(&loader.loader_dir));
    let jvm = loader
        .jvm_arguments
        .iter()
        .map(|arg| {
            let arg = arg
                .replace("${version_name}", &input.version.id)
                .replace("${library_directory}", &library_directory)
                .replace("${classpath_separator}", &separator);
            patches::patch_argument(&arg, Platform::current())
        })
        .collect();
    (jvm, loader.game_arguments.clone())
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authenticator::{AccountMeta, AccountProfile, AccountType, Ownership, XboxAccount};

    fn account() -> Account {
        Account {
            access_token: "token".into(),
            client_token: "client".into(),
            uuid: "uuid".into(),
            name: "Player".into(),
            refresh_token: Some("refresh".into()),
            user_properties: "{}".into(),
            meta: AccountMeta::xbox(0, Ownership::Owned, vec![]),
            xbox_account: Some(XboxAccount {
                xuid: Some("xuid".into()),
                gamertag: None,
                age_group: None,
            }),
            profile: AccountProfile::default(),
            client_id: None,
            user_info: None,
        }
    }

    fn offline_account() -> Account {
        let mut account = crate::authenticator::Yggdrasil::offline("Steve");
        account.access_token = "offline-token".into();
        account.client_token = "offline-client".into();
        account
    }

    fn modern_version() -> VersionJson {
        serde_json::from_str(
            r#"{
            "id": "1.20.1", "type": "release", "mainClass": "net.minecraft.client.main.Main",
            "arguments": {
                "game": ["--username", "${auth_player_name}", "--version", "${version_name}", "--uuid", "${auth_uuid}",
                         {"rules": [{"action": "allow", "features": {"is_demo_user": true}}], "value": "--demo"},
                         "--accessToken", "${auth_access_token}", "--clientId", "${clientid}", "--xuid", "${auth_xuid}", "--userType", "${user_type}"],
                "jvm": ["-Djava.library.path=${natives_directory}", "-cp", "${classpath}"],
                "default-user-jvm": [
                    {"value": ["-Xms2G", "-XX:+UseStringDeduplication"]},
                    {"rules": [{"action": "allow", "os": {"name": "osx"}}, {"action": "allow", "os": {"name": "linux"}}], "value": ["-XX:+UseZGC"]}
                ]
            },
            "assetIndex": {"id": "5", "sha1": "a", "size": 1, "url": "u"},
            "assets": "5",
            "downloads": {"client": {"sha1": "c", "size": 1, "url": "cu"}},
            "libraries": [
                {"name": "org.lwjgl:lwjgl:3.3.1", "downloads": {"artifact": {"path": "p", "sha1": "s", "size": 1, "url": "u"}}},
                {"name": "org.lwjgl:lwjgl:3.3.1:natives-windows", "downloads": {"artifact": {"path": "p", "sha1": "s", "size": 1, "url": "u"}}, "rules": [{"action": "allow", "os": {"name": "windows"}}]},
                {"name": "com.mojang:logging:1.1.1", "downloads": {"artifact": {"path": "p", "sha1": "s", "size": 1, "url": "u"}}}
            ],
            "logging": {"client": {"argument": "-Dlog4j.configurationFile=${path}", "file": {"id": "client-1.12.xml", "sha1": "x", "size": 1, "url": "u"}, "type": "log4j2-xml"}}
        }"#,
        )
        .unwrap()
    }

    fn forge_loader() -> LoaderJson {
        let raw = serde_json::json!({
            "id": "1.20.1-forge-47.2.0",
            "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
            "arguments": {
                "game": ["--launchTarget", "forgeclient", "--fml.forgeVersion", "47.2.0"],
                "jvm": ["-DignoreList=bootstraplauncher,${version_name}.jar", "-DlibraryDirectory=${library_directory}", "-p", "${library_directory}/a.jar${classpath_separator}${library_directory}/b.jar"]
            },
            "libraries": [
                {"name": "cpw.mods:securejarhandler:2.1.10"},
                {"name": "org.ow2.asm:asm:9.5"},
                {"name": "org.apache.logging.log4j:log4j-slf4j2-impl:2.19.0"},
                {"name": "com.mojang:logging:1.1.1"}
            ]
        });
        LoaderJson::from_value(raw, PathBuf::from("/root/loader")).unwrap()
    }

    fn options() -> LaunchOptions {
        LaunchOptions::new("/root", "1.20.1")
    }

    #[test]
    fn game_arguments_drop_objects_and_replace_exact_placeholders() {
        let version = modern_version();
        let account = account();
        let options = options();
        let input = ArgumentsInput {
            version: &version,
            loader: None,
            account: &account,
            options: &options,
            natives_list: false,
        };
        let game = game_arguments(&input);
        assert_eq!(
            game,
            vec![
                "--username",
                "Player",
                "--version",
                "1.20.1",
                "--uuid",
                "uuid",
                "--accessToken",
                "token",
                "--clientId",
                "client",
                "--xuid",
                "xuid",
                "--userType",
                "msa"
            ]
        );
    }

    #[test]
    fn game_arguments_merge_and_dedupe_legacy_loader_arguments() {
        let version: VersionJson = serde_json::from_str(
            r#"{"id": "1.12.2", "type": "release", "mainClass": "m",
                "minecraftArguments": "--username ${auth_player_name} --version ${version_name} --userType ${user_type} --versionType ${version_type}",
                "assets": "1.12", "downloads": {}, "libraries": []}"#,
        )
        .unwrap();
        let raw = serde_json::json!({
            "id": "1.12.2-forge-14.23.5.2860",
            "mainClass": "net.minecraft.launchwrapper.Launch",
            "minecraftArguments": "--username ${auth_player_name} --version ${version_name} --userType ${user_type} --tweakClass net.minecraftforge.fml.common.launcher.FMLTweaker --versionType Forge",
            "libraries": []
        });
        let loader = LoaderJson::from_value(raw, PathBuf::from("/root/loader")).unwrap();
        let account = account();
        let options = LaunchOptions::new("/root", "1.12.2");
        let input = ArgumentsInput {
            version: &version,
            loader: Some(&loader),
            account: &account,
            options: &options,
            natives_list: true,
        };
        let game = game_arguments(&input);
        assert_eq!(
            game,
            vec![
                "--username",
                "Player",
                "--version",
                "1.12.2-forge-14.23.5.2860",
                "--userType",
                "msa",
                "--versionType",
                "release",
                "--tweakClass",
                "net.minecraftforge.fml.common.launcher.FMLTweaker",
                "Forge"
            ]
        );
    }

    #[test]
    fn user_type_is_xbox_for_1_16() {
        let version: VersionJson = serde_json::from_str(
            r#"{"id": "1.16.5", "type": "release", "mainClass": "m", "arguments": {"game": ["${user_type}"]}, "downloads": {}, "libraries": []}"#,
        )
        .unwrap();
        let account = account();
        let options = LaunchOptions::new("/root", "1.16.5");
        let input = ArgumentsInput {
            version: &version,
            loader: None,
            account: &account,
            options: &options,
            natives_list: false,
        };
        assert_eq!(game_arguments(&input), vec!["Xbox"]);
    }

    #[test]
    fn jvm_arguments_follow_node_order() {
        let version = modern_version();
        let account = account();
        let mut options = options();
        options.jvm_args = vec!["-Dcustom=1".into()];
        let input = ArgumentsInput {
            version: &version,
            loader: None,
            account: &account,
            options: &options,
            natives_list: false,
        };
        let jvm = jvm_arguments(&input);
        assert_eq!(jvm[0], "-Xms1G");
        assert_eq!(jvm[1], "-Xmx2G");
        assert_eq!(jvm[2], "-XX:+UnlockExperimentalVMOptions");
        assert!(!jvm.contains(&"-XX:+UseG1GC".to_owned()));
        assert!(jvm.contains(&"-Djna.tmpdir=/root/versions/1.20.1/natives".to_owned()));
        assert!(!jvm.iter().any(|a| a.starts_with("-Djava.library.path=")));
        assert!(jvm.contains(
            &"-Dlog4j.configurationFile=/root/assets/log_configs/client-1.12.xml".to_owned()
        ));
        assert!(jvm.contains(&"-XX:+UseStringDeduplication".to_owned()));
        assert!(!jvm.contains(&"-Xms2G".to_owned()));
        assert!(!jvm.contains(&"-XX:+UseZGC".to_owned()));
        assert_eq!(jvm.last().unwrap(), "-Dcustom=1");
        let os_specific = match Platform::current() {
            Platform::MacOs => "-XstartOnFirstThread",
            Platform::Linux => "-Xss1M",
            Platform::Windows => {
                "-XX:HeapDumpPath=MojangTricksIntelDriversForPerformance_javaw.exe_minecraft.exe.heapdump"
            }
        };
        assert_eq!(jvm[11], os_specific);
    }

    #[test]
    fn arg_keys_match_node() {
        assert_eq!(arg_key("-Xmx2G"), "-Xmx");
        assert_eq!(arg_key("-Xss1M"), "-Xss");
        assert_eq!(arg_key("-XX:+UseZGC"), "-XX:+UseZGC");
        assert_eq!(arg_key("-Dfoo=bar"), "-Dfoo");
        assert_eq!(arg_key("-XX:G1HeapRegionSize=32M"), "-XX:G1HeapRegionSize");
        assert_eq!(arg_key("-XstartOnFirstThread"), "-XstartOnFirstThread");
        assert_eq!(arg_key("2G"), "2G");
    }

    #[test]
    fn classpath_puts_loader_first_and_skips_slf4j2_impl() {
        let version = modern_version();
        let loader = forge_loader();
        let account = account();
        let options = options();
        let input = ArgumentsInput {
            version: &version,
            loader: Some(&loader),
            account: &account,
            options: &options,
            natives_list: false,
        };
        let (cp, main_class) = classpath(&input);
        assert_eq!(
            main_class.as_deref(),
            Some("cpw.mods.bootstraplauncher.BootstrapLauncher")
        );
        let separator = Platform::current().classpath_separator();
        let entries: Vec<&str> = cp[1].split(separator).collect();
        assert_eq!(
            entries[0],
            "/root/loader/libraries/cpw/mods/securejarhandler/2.1.10/securejarhandler-2.1.10.jar"
        );
        assert_eq!(
            entries[1],
            "/root/loader/libraries/org/ow2/asm/asm/9.5/asm-9.5.jar"
        );
        assert!(!entries.iter().any(|e| e.contains("slf4j2-impl")));
        assert_eq!(
            entries[2],
            "/root/loader/libraries/com/mojang/logging/1.1.1/logging-1.1.1.jar"
        );
        assert_eq!(
            entries[3],
            "/root/libraries/org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1.jar"
        );
        if Platform::current() != Platform::Windows {
            assert!(!entries.iter().any(|e| e.contains("natives-windows")));
        }
        assert_eq!(*entries.last().unwrap(), "/root/versions/1.20.1/1.20.1.jar");
    }

    #[test]
    fn loader_arguments_substitute_placeholders() {
        let version = modern_version();
        let loader = forge_loader();
        let account = account();
        let options = options();
        let input = ArgumentsInput {
            version: &version,
            loader: Some(&loader),
            account: &account,
            options: &options,
            natives_list: false,
        };
        let (jvm, game) = loader_arguments(&input);
        let separator = Platform::current().classpath_separator();
        assert_eq!(jvm[0], "-DignoreList=bootstraplauncher,1.20.1.jar");
        assert_eq!(jvm[1], "-DlibraryDirectory=/root/loader/libraries");
        assert_eq!(
            jvm[3],
            format!("/root/loader/libraries/a.jar{separator}/root/loader/libraries/b.jar")
        );
        assert_eq!(
            game,
            vec![
                "--launchTarget",
                "forgeclient",
                "--fml.forgeVersion",
                "47.2.0"
            ]
        );
    }

    #[test]
    fn full_plan_follows_node_assembly_order() {
        let version = modern_version();
        let loader = forge_loader();
        let account = account();
        let options = options();
        let input = ArgumentsInput {
            version: &version,
            loader: Some(&loader),
            account: &account,
            options: &options,
            natives_list: false,
        };
        let plan = build(&input, PathBuf::from("/java")).unwrap();
        let cp = plan.args.iter().position(|a| a == "-cp").unwrap();
        let ignore = plan
            .args
            .iter()
            .position(|a| a.starts_with("-DignoreList="))
            .unwrap();
        let main = plan
            .args
            .iter()
            .position(|a| a == "cpw.mods.bootstraplauncher.BootstrapLauncher")
            .unwrap();
        let username = plan.args.iter().position(|a| a == "--username").unwrap();
        let target = plan
            .args
            .iter()
            .position(|a| a == "--launchTarget")
            .unwrap();
        assert!(cp < ignore && ignore < main && main < username && username < target);
        assert_eq!(plan.args[0], "-Xms1G");
        assert!(plan.redacted_command(&["token"]).contains("????????"));
    }

    #[test]
    fn mcp_replaces_the_client_jar() {
        let version = modern_version();
        let account = account();
        let mut options = options();
        options.mcp = Some("mcp/client.jar".into());
        let input = ArgumentsInput {
            version: &version,
            loader: None,
            account: &account,
            options: &options,
            natives_list: false,
        };
        let (cp, _) = classpath(&input);
        assert!(cp[1].ends_with("/root/mcp/client.jar"));
    }

    #[test]
    fn user_type_and_placeholders_follow_the_account_kind() {
        let version: VersionJson = serde_json::from_str(
            r#"{"id": "1.20.1", "type": "release", "mainClass": "m", "arguments": {"game": ["${user_type}", "${auth_xuid}", "${clientid}"]}, "downloads": {}, "libraries": []}"#,
        )
        .unwrap();
        let options = LaunchOptions::new("/root", "1.20.1");

        let offline = offline_account();
        let input = ArgumentsInput {
            version: &version,
            loader: None,
            account: &offline,
            options: &options,
            natives_list: false,
        };
        assert_eq!(
            game_arguments(&input),
            vec!["Mojang", "offline-token", "offline-client"]
        );

        let mut azauth = offline_account();
        azauth.meta.kind = AccountType::AzAuth;
        azauth.client_id = Some("azuriom-client".into());
        let input = ArgumentsInput {
            version: &version,
            loader: None,
            account: &azauth,
            options: &options,
            natives_list: false,
        };
        assert_eq!(
            game_arguments(&input),
            vec!["AZauth", "offline-token", "azuriom-client"]
        );

        let xbox = account();
        let input = ArgumentsInput {
            version: &version,
            loader: None,
            account: &xbox,
            options: &options,
            natives_list: false,
        };
        assert_eq!(game_arguments(&input), vec!["msa", "xuid", "client"]);
    }
}
