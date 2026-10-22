//! Resolve a verified GOG installation: the play task its `goggame-*.info`
//! file names, and what its build runs once before its game first starts, as
//! heroic-gogdl's `launch.py` and Heroic's `setup.ts` read them. Process
//! execution remains in the native host.
use std::path::{Path, PathBuf};

use obelisk_core::{
    install::read_file_bounded,
    installation::{self, InstallFailure},
    launch::{self, LaunchTarget},
    platform::Platform,
};
use serde::Deserialize;

use super::{Installation, valid_product_id};
use crate::{GogError, manifest::SUPPORT_DIRECTORY};

pub(super) const MAX_INFO_BYTES: usize = 1024 * 1024;

/// The game an installation starts, from its receipt and its own files alone.
/// Nothing is fetched.
pub struct InstalledLaunch {
    pub target: LaunchTarget,
    installation: Installation,
    /// The folder the build's files are in.
    root: PathBuf,
}

/// A program run once in a Windows build's prefix before its game first
/// starts, with its Windows command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prerequisite {
    pub id: String,
    pub program: PathBuf,
    pub arguments: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GameInfo {
    #[serde(default)]
    play_tasks: Vec<PlayTask>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlayTask {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    is_primary: bool,
    #[serde(default)]
    is_hidden: bool,
    #[serde(default)]
    category: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    working_dir: String,
    #[serde(default)]
    arguments: String,
}

/// What the installation in `directory` starts, from its receipt alone.
pub fn installed_launch(
    product_id: &str,
    platform: Platform,
    directory: &Path,
) -> Result<InstalledLaunch, GogError> {
    if !valid_product_id(product_id) {
        return Err(GogError::Protocol);
    }
    let installed =
        installation::open::<Installation>(directory).map_err(|failure| match failure {
            InstallFailure::Io(_) => launch_error("directoryUnreadable"),
            failure => failure.into(),
        })?;
    let installation = installed
        .receipt
        .ok_or_else(|| launch_error("incompleteInstallation"))?;
    if installation.product_id != product_id || installation.platform != platform {
        return Err(launch_error("incompleteInstallation"));
    }
    let root = match installation.content_root.as_str() {
        "" => installed.root.clone(),
        content => installed
            .root
            .join(obelisk_core::install::safe_relative_path(content)?),
    };
    let task = play_task(&root, product_id)?;
    let path = task.path.replace('\\', "/");
    let working_directory = match task.working_dir.replace('\\', "/") {
        directory if !directory.is_empty() => directory,
        _ => Path::new(&path)
            .parent()
            .and_then(Path::to_str)
            .unwrap_or("")
            .to_owned(),
    };
    let target = launch::resolve(platform, &root, &path, &task.arguments, &working_directory)
        .map_err(|error| launch_error(error.key_suffix()))?;
    Ok(InstalledLaunch {
        target,
        installation,
        root,
    })
}

/// Where a build keeps the file that names its play tasks, from the folder
/// its files are in: that folder, or its bundle's resources.
pub(super) fn info_files(product_id: &str) -> [String; 2] {
    let name = format!("goggame-{product_id}.info");
    [name.clone(), format!("Contents/Resources/{name}")]
}

/// The task Galaxy plays: the primary one, else the first game.
fn play_task(root: &Path, product_id: &str) -> Result<PlayTask, GogError> {
    let bytes = info_files(product_id)
        .iter()
        .find_map(|path| {
            read_file_bounded(&root.join(path), MAX_INFO_BYTES)
                .ok()
                .flatten()
        })
        .ok_or_else(|| launch_error("noPlayTask"))?;
    primary_task(&bytes)
}

/// The program the task Galaxy plays starts, from the folder the build's
/// files are in, as its `goggame-*.info` file's `bytes` name it.
pub(super) fn played_program(bytes: &[u8]) -> Option<String> {
    primary_task(bytes)
        .ok()
        .map(|task| task.path.replace('\\', "/"))
}

/// The primary task of an info file, else its first game.
fn primary_task(bytes: &[u8]) -> Result<PlayTask, GogError> {
    let info: GameInfo = serde_json::from_slice(bytes).map_err(|_| launch_error("noPlayTask"))?;
    let mut tasks: Vec<PlayTask> = info
        .play_tasks
        .into_iter()
        .filter(|task| task.kind == "FileTask" && !task.path.is_empty())
        .collect();
    let index = tasks
        .iter()
        .position(|task| task.is_primary)
        .or_else(|| tasks.iter().position(|task| task.category == "game"))
        .or_else(|| tasks.iter().position(|task| !task.is_hidden))
        .ok_or_else(|| launch_error("noPlayTask"))?;
    Ok(tasks.swap_remove(index))
}

impl InstalledLaunch {
    /// What a Windows build runs once in its prefix before its game first
    /// starts, in order: its installer scripts, then its redistributables.
    /// `windows_path` names a Mac path as Wine sees it.
    pub fn prerequisites(&self, windows_path: impl Fn(&Path) -> String) -> Vec<Prerequisite> {
        let installation = &self.installation;
        if installation.platform != Platform::Windows {
            return Vec::new();
        }
        let language = &installation.language;
        let name = language_name(language);
        let switches = |product_id: &str| {
            format!(
                "/VERYSILENT \"/DIR={}\" /Language={name} /LANG={name} /ProductId={product_id} \
                 /galaxyclient /buildId={} /versionName={} /lang-code={language} \
                 /nodesktopshorctut /nodesktopshortcut",
                windows_path(&self.root),
                installation.build_id,
                quoted(&installation.version_name),
            )
        };
        let mut prerequisites = Vec::new();
        for product in &installation.products {
            let support = self.root.join(SUPPORT_DIRECTORY).join(&product.product_id);
            if installation.script_interpreter {
                prerequisites.push(Prerequisite {
                    id: format!("isi:{}:{}", product.product_id, installation.build_id),
                    program: self.root.join("__redist/ISI/scriptinterpreter.exe"),
                    arguments: format!(
                        "{} \"/supportDir={}\"",
                        switches(&product.product_id),
                        windows_path(&support)
                    ),
                });
            } else if let Some(installer) = inside(&support, &product.temp_executable) {
                prerequisites.push(Prerequisite {
                    id: format!("installer:{}:{}", product.product_id, installation.build_id),
                    program: installer,
                    arguments: switches(&product.product_id),
                });
            }
        }
        prerequisites.extend(installation.dependencies.iter().filter_map(|dependency| {
            Some(Prerequisite {
                id: dependency.id.clone(),
                program: inside(&self.root, &dependency.path)?,
                arguments: dependency.arguments.clone(),
            })
        }));
        prerequisites
    }
}

/// `relative` inside `root`, unless it is empty or leaves it.
fn inside(root: &Path, relative: &str) -> Option<PathBuf> {
    obelisk_core::install::safe_relative_path(relative)
        .ok()
        .map(|relative| root.join(relative))
}

/// A switch value with spaces, quoted for Windows.
fn quoted(value: &str) -> String {
    match value.contains([' ', '\t']) {
        true => format!("\"{}\"", value.replace('"', "")),
        false => value.to_owned(),
    }
}

/// The English name GOG's installer scripts take for a language.
fn language_name(language: &str) -> &'static str {
    let base = language
        .split(['-', '_'])
        .next()
        .unwrap_or(language)
        .to_ascii_lowercase();
    match base.as_str() {
        "fr" => "French",
        "de" => "German",
        "es" => "Spanish",
        "it" => "Italian",
        "pt" if language.eq_ignore_ascii_case("pt-BR") => "Brazilian-Portuguese",
        "pt" => "Portuguese",
        "pl" => "Polish",
        "ru" => "Russian",
        "cs" | "cz" => "Czech",
        "hu" => "Hungarian",
        "nl" => "Dutch",
        "tr" => "Turkish",
        "ja" | "jp" => "Japanese",
        "ko" => "Korean",
        "zh" => "Chinese",
        _ => "English",
    }
}

fn launch_error(suffix: &str) -> GogError {
    GogError::Service {
        message: format!("backend.gog.launch.{suffix}"),
    }
}

#[cfg(test)]
mod tests;
