use crate::vite_plus::{self, LAUNCH_SCRIPT, Options};
use log::debug;
use std::{collections::BTreeMap, env};
use zed_extension_api::serde_json::{Value, json};
use zed_extension_api::settings::LspSettings;
use zed_extension_api::{
    Command, EnvVars, LanguageServerId, LanguageServerInstallationStatus, Result, Worktree,
    node_binary_path, npm_install_package, npm_package_installed_version,
    npm_package_latest_version, set_language_server_installation_status,
};

pub const OXLINT_SERVER_ID: &str = "oxlint";
pub const OXFMT_SERVER_ID: &str = "oxfmt";

pub trait ZedLspSupport {
    fn package_name(&self) -> &'static str;
    fn sources(&self) -> &BTreeMap<u64, bool>;
    fn sources_mut(&mut self) -> &mut BTreeMap<u64, bool>;

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Command> {
        // Restarts recheck settings and installs, including after startup failures.
        self.sources_mut().remove(&worktree.id());
        let settings = LspSettings::for_worktree(language_server_id.as_ref(), worktree)?;
        let env =
            server_env(&settings, worktree.shell_env(), &worktree.root_path(), self.package_name());

        // Honor complete overrides before resolving Node or downloading a fallback.
        if let Some(command) = custom_command(&settings, env.clone())? {
            self.sources_mut().insert(worktree.id(), false);
            return Ok(command);
        }

        let options = Options::from_initialization(settings.initialization_options.as_ref())?;
        let node = node_binary_path()?;
        let directories = vite_plus::inspect(
            &node,
            "ancestors",
            &worktree.root_path(),
            self.package_name(),
            &env,
        )?;
        let directories = directories.as_array().ok_or("Expected a list of project directories")?;

        if let Some(project) = vite_plus::detect_project(directories, &options) {
            let executable = if let Some(path) = &project.vp_path {
                // Explicit relative paths are relative to the opened worktree.
                vite_plus::inspect(&node, "executable", &worktree.root_path(), path, &env)?
            } else {
                vite_plus::inspect(&node, "global", &worktree.root_path(), "vp", &env)?
            };
            let path = executable["path"].as_str().ok_or_else(|| {
                format!(
                    "Vite+ selected for {} but vp was not found. Install dependencies (for example, pnpm install), or set initialization_options.vpPath, then restart the language server.",
                    project.root
                )
            })?;
            let loader = if executable["node"] == true { "node" } else { "native" };
            let tool = if self.package_name() == OXLINT_SERVER_ID { "lint" } else { "fmt" };
            debug!("Starting vp {tool} --lsp from {path} in {}", project.root);
            let command = Command {
                command: node,
                args: ["-e", LAUNCH_SCRIPT, "--", &project.root, path, loader, tool]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                env,
            };
            self.sources_mut().insert(worktree.id(), true);
            return Ok(command);
        }

        let path = vite_plus::standalone_path(directories, self.package_name());
        let path = if let Some(path) = path {
            path.to_owned()
        } else {
            self.update_extension_language_server_if_outdated(language_server_id)?;
            env::current_dir()
                .map_err(|err| err.to_string())?
                .join("node_modules")
                .join(self.package_name())
                .join("bin")
                .join(self.package_name())
                .to_string_lossy()
                .into_owned()
        };
        debug!("Starting {} --lsp from {path}", self.package_name());
        self.sources_mut().insert(worktree.id(), false);
        Ok(Command { command: node, args: vec![path, "--lsp".into()], env })
    }

    fn uses_vite_plus(&self, settings: &LspSettings, worktree: &Worktree) -> Result<bool> {
        if let Some(source) = self.sources().get(&worktree.id()) {
            // Configuration follows the running command until a server restart.
            return Ok(*source);
        }
        if custom_command(settings, Vec::new())?.is_some() {
            return Ok(false);
        }
        // Zed may request configuration before requesting a command.
        let options = Options::from_initialization(settings.initialization_options.as_ref())?;
        let directories = vite_plus::inspect(
            &node_binary_path()?,
            "ancestors",
            &worktree.root_path(),
            self.package_name(),
            &worktree.shell_env(),
        )?;
        Ok(vite_plus::detect_project(
            directories.as_array().ok_or("Expected a list of project directories")?,
            &options,
        )
        .is_some())
    }

    fn language_server_initialization_options(
        &self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Option<Value>> {
        let settings = LspSettings::for_worktree(language_server_id.as_ref(), worktree)?;
        let vite_plus = self.uses_vite_plus(&settings, worktree)?;
        Ok(initialization_options(settings.initialization_options, self.package_name(), vite_plus))
    }

    fn language_server_workspace_configuration(
        &self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Option<Value>> {
        let settings = LspSettings::for_worktree(language_server_id.as_ref(), worktree)?;
        let vite_plus = self.uses_vite_plus(&settings, worktree)?;
        Ok(workspace_configuration(settings, self.package_name(), vite_plus))
    }

    fn update_extension_language_server_if_outdated(
        &self,
        language_server_id: &LanguageServerId,
    ) -> Result<()> {
        set_language_server_installation_status(
            language_server_id,
            &LanguageServerInstallationStatus::CheckingForUpdate,
        );
        let package_name = self.package_name();
        let current_version = npm_package_installed_version(package_name)?;
        let latest_version = npm_package_latest_version(package_name)?;
        if current_version.as_deref() != Some(latest_version.as_str()) {
            set_language_server_installation_status(
                language_server_id,
                &LanguageServerInstallationStatus::Downloading,
            );
            npm_install_package(package_name, &latest_version)?;
        }
        set_language_server_installation_status(
            language_server_id,
            &LanguageServerInstallationStatus::None,
        );
        Ok(())
    }
}

fn custom_command(settings: &LspSettings, env: EnvVars) -> Result<Option<Command>> {
    let Some(binary) = &settings.binary else {
        return Ok(None);
    };
    match (&binary.path, &binary.arguments) {
        (Some(path), Some(args)) => {
            Ok(Some(Command { command: path.clone(), args: args.clone(), env }))
        }
        (None, None) => Ok(None),
        _ => {
            Err("When supplying binary.arguments, binary.path must be supplied (or vice-versa)."
                .into())
        }
    }
}

fn server_env(settings: &LspSettings, shell: EnvVars, root: &str, tool: &str) -> EnvVars {
    let mut env = BTreeMap::new();
    let overrides = settings.binary.as_ref().and_then(|binary| binary.env.as_ref());
    for (key, value) in shell.into_iter().chain(
        overrides.into_iter().flat_map(|env| env.iter().map(|(k, v)| (k.clone(), v.clone()))),
    ) {
        env.insert(if key.eq_ignore_ascii_case("PATH") { "PATH".into() } else { key }, value);
    }
    if tool == OXLINT_SERVER_ID
        && let Some(path) = env.get_mut("OXLINT_TSGOLINT_PATH")
    {
        // WASI paths use Unix syntax even when the host is Windows.
        let absolute = path.starts_with('/')
            || path.starts_with('\\')
            || path.as_bytes().get(1) == Some(&b':');
        if !absolute {
            *path = format!("{root}/{path}");
        }
    }
    env.into_iter().collect()
}

fn initialization_options(
    mut options: Option<Value>,
    tool: &str,
    vite_plus: bool,
) -> Option<Value> {
    if let Some(Value::Object(options)) = options.as_mut() {
        options.remove("binarySource");
        options.remove("vpPath");
    }
    if vite_plus {
        let options = options.get_or_insert_with(|| json!({}));
        if !options.is_object() {
            *options = json!({});
        }
        force_nested_config(
            options.as_object_mut().unwrap().entry("settings").or_insert_with(|| json!({})),
            tool,
        );
    }
    options
}

fn workspace_configuration(settings: LspSettings, tool: &str, vite_plus: bool) -> Option<Value> {
    let options = initialization_options(settings.initialization_options, tool, vite_plus);
    let mut config = options.and_then(|v| v.get("settings").cloned());
    if let Some(settings) = settings.settings {
        let target = config.get_or_insert_with(|| json!({}));
        if let (Some(target), Some(settings)) = (target.as_object_mut(), settings.as_object()) {
            target.extend(settings.clone());
        } else {
            *target = settings;
        }
    }
    if vite_plus {
        force_nested_config(config.get_or_insert_with(|| json!({})), tool);
    }
    config
}

fn force_nested_config(settings: &mut Value, tool: &str) {
    if !settings.is_object() {
        *settings = json!({});
    }
    let key =
        if tool == OXLINT_SERVER_ID { "disableNestedConfig" } else { "fmt.disableNestedConfig" };
    settings[key] = true.into();
}

#[cfg(test)]
mod tests {
    use super::*;
    use zed_extension_api::serde_json::from_value;

    #[test]
    fn custom_commands_override_invalid_source_settings_and_allow_env_only_settings() {
        let settings = from_value(json!({
            "binary": {"path":"/custom/oxlint", "arguments":["--lsp"]},
            "initialization_options": {"binarySource":"invalid", "vpPath":"missing"}
        }))
        .unwrap();
        let command =
            custom_command(&settings, vec![("TEST".into(), "value".into())]).unwrap().unwrap();
        assert_eq!(command.command, "/custom/oxlint");
        assert_eq!(command.args, ["--lsp"]);
        assert_eq!(command.env, [("TEST".into(), "value".into())]);
        for binary in [json!({"path":"oxlint"}), json!({"arguments":["--lsp"]})] {
            assert!(
                custom_command(&from_value(json!({"binary":binary})).unwrap(), vec![]).is_err()
            );
        }
        assert!(
            custom_command(
                &from_value(json!({"binary":{"env":{"TEST":"value"}}})).unwrap(),
                vec![]
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn vite_plus_overrides_nested_config_at_initialization_and_configuration() {
        for (tool, key) in [
            (OXLINT_SERVER_ID, "disableNestedConfig"),
            (OXFMT_SERVER_ID, "fmt.disableNestedConfig"),
        ] {
            let original = json!({"binarySource":"vite-plus", "vpPath":"/custom/vp", "settings":{key:false, "run":"onSave", "configPath":"custom.json"}});
            let options = initialization_options(Some(original.clone()), tool, true).unwrap();
            assert_eq!(options["settings"][key], true);
            assert_eq!(options["settings"]["run"], "onSave");
            assert!(options.get("binarySource").is_none());
            assert!(options.get("vpPath").is_none());
            assert_eq!(original["settings"][key], false);
            let config = workspace_configuration(from_value(json!({"initialization_options":original, "settings":{key:false, "run":"onType"}})).unwrap(), tool, true).unwrap();
            assert_eq!(config[key], true);
            assert_eq!(config["run"], "onType");
            assert_eq!(config["configPath"], "custom.json");
            for value in [None, Some(Value::Null), Some(json!({"settings":null}))] {
                assert_eq!(
                    initialization_options(value, tool, true).unwrap()["settings"][key],
                    true
                );
            }
        }
    }

    #[test]
    fn standalone_configuration_preserves_user_values() {
        let original =
            json!({"settings":{"disableNestedConfig":false, "fmt.disableNestedConfig":false}});
        assert_eq!(
            initialization_options(Some(original.clone()), OXLINT_SERVER_ID, false),
            Some(original.clone())
        );
        assert_eq!(
            workspace_configuration(
                from_value(json!({"initialization_options":original})).unwrap(),
                OXLINT_SERVER_ID,
                false
            ),
            Some(original["settings"].clone())
        );
        assert_eq!(initialization_options(None, OXLINT_SERVER_ID, false), None);
    }

    #[test]
    fn tool_sources_are_separate_for_each_worktree() {
        let mut lint = crate::oxlint::ZedOxlintLsp::default();
        let mut fmt = crate::oxfmt::ZedOxfmtLsp::default();
        lint.sources_mut().insert(1, true);
        lint.sources_mut().insert(2, false);
        fmt.sources_mut().insert(1, false);
        assert_eq!(lint.sources().get(&1), Some(&true));
        assert_eq!(lint.sources().get(&2), Some(&false));
        assert_eq!(fmt.sources().get(&1), Some(&false));
    }

    #[test]
    fn environment_overrides_preserve_shell_and_resolve_tsgolint_paths() {
        let settings = from_value(json!({"binary":{"env":{"Path":"/custom/bin", "OXLINT_TSGOLINT_PATH":"tools/tsgolint"}}})).unwrap();
        let shell =
            vec![("PATH".into(), "/shell/bin".into()), ("SHELL_SETTING".into(), "kept".into())];
        let env: BTreeMap<_, _> =
            server_env(&settings, shell, "/repo", OXLINT_SERVER_ID).into_iter().collect();
        assert_eq!(env["PATH"], "/custom/bin");
        assert_eq!(env["SHELL_SETTING"], "kept");
        assert_eq!(env["OXLINT_TSGOLINT_PATH"], "/repo/tools/tsgolint");
        assert!(!env.contains_key("Path"));
        for path in ["/absolute/tsgolint", r"C:\tools\tsgolint.exe", r"\\server\share\tsgolint.exe"]
        {
            let settings =
                from_value(json!({"binary":{"env":{"OXLINT_TSGOLINT_PATH":path}}})).unwrap();
            assert_eq!(
                server_env(&settings, vec![], "/repo", OXLINT_SERVER_ID),
                [("OXLINT_TSGOLINT_PATH".into(), path.into())]
            );
        }
    }
}
