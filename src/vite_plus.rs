use zed_extension_api::serde_json::{Value, from_slice};
use zed_extension_api::{EnvVars, Result, process};

pub const PROJECT_SCRIPT: &str = include_str!("project.js");
pub const LAUNCH_SCRIPT: &str = include_str!("launch_vp.js");

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BinarySource {
    #[default]
    Auto,
    VitePlus,
    Oxc,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub source: BinarySource,
    pub vp_path: Option<String>,
}

impl Options {
    pub fn from_initialization(options: Option<&Value>) -> Result<Self> {
        let source = match options.and_then(|v| v.get("binarySource")) {
            None | Some(Value::Null) => BinarySource::Auto,
            Some(Value::String(value)) if value == "auto" => BinarySource::Auto,
            Some(Value::String(value)) if value == "vite-plus" => BinarySource::VitePlus,
            Some(Value::String(value)) if value == "oxc" => BinarySource::Oxc,
            _ => return Err("binarySource must be auto, vite-plus, or oxc.".into()),
        };
        if source == BinarySource::Oxc {
            return Ok(Self { source, vp_path: None });
        }
        let vp_path = match options.and_then(|v| v.get("vpPath")) {
            None | Some(Value::Null) => None,
            Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
            _ => return Err("vpPath must be a non-empty executable path.".into()),
        };
        Ok(Self { source, vp_path })
    }
}

pub fn standalone_path<'a>(directories: &'a [Value], package: &str) -> Option<&'a str> {
    let index = directories.iter().position(|dir| declares_package(&dir["package"], package))?;
    directories[index..].iter().find_map(|dir| dir["standalone"].as_str())
}

#[derive(Debug, PartialEq, Eq)]
pub struct Project {
    pub root: String,
    pub vp_path: Option<String>,
}

pub fn declares_package(package: &Value, name: &str) -> bool {
    ["dependencies", "devDependencies"]
        .iter()
        .any(|key| package[*key][name].as_str().is_some_and(|version| !version.is_empty()))
}

/// Port of RFC #1614's identity and local resolution phases. The filesystem
/// bridge supplies ancestors from the worktree through the monorepo root.
pub fn detect_project(directories: &[Value], options: &Options) -> Option<Project> {
    if options.source == BinarySource::Oxc {
        return None;
    }
    let forced = options.source == BinarySource::VitePlus || options.vp_path.is_some();
    let index = if forced {
        directories.iter().position(|dir| dir["package"].is_object()).unwrap_or(0)
    } else {
        directories.iter().position(|dir| declares_package(&dir["package"], "vite-plus"))?
    };
    let root = directories.get(index)?["root"].as_str()?.to_owned();
    let vp_path = options.vp_path.clone().or_else(|| {
        directories[index..].iter().find_map(|dir| dir["vp"].as_str().map(str::to_owned))
    });
    Some(Project { root, vp_path })
}

pub fn inspect(node: &str, mode: &str, path: &str, tool: &str, env: &EnvVars) -> Result<Value> {
    let output = process::Command::new(node)
        .args(["-e", PROJECT_SCRIPT, "--", mode, path, tool])
        .envs(env.clone())
        .output()?;
    if output.status != Some(0) {
        return Err(format!(
            "Could not inspect the {tool} installation: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    from_slice(&output.stdout).map_err(|err| format!("Invalid installation information: {err}"))
}

#[cfg(test)]
mod tests;
