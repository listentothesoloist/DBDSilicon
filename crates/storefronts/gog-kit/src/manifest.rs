//! GOG content system generation 2 documents: a build's manifest, which names
//! its depots and redistributables, the redistributables' repository, and each
//! depot's manifest, which names its files and their chunks. They arrive
//! zlib-compressed from the CDN.
use std::collections::{HashMap, HashSet, hash_map::Entry};

use obelisk_core::install::safe_relative_path;
use serde::{Deserialize, Serialize};

use crate::GogError;

/// Chunks are cut at 10 MiB, but a file's last one may reach twice that.
pub(crate) const MAX_CHUNK_BYTES: u64 = 32 * 1024 * 1024;
/// Where Galaxy keeps a product's support files, which only its installer
/// scripts read.
pub(crate) const SUPPORT_DIRECTORY: &str = "gog-support";

/// One build of a product, as `builds?generation=2` lists it.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Build {
    /// The build's identity, which its manifest records too.
    #[serde(default)]
    pub build_id: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub generation: u32,
    #[serde(default)]
    pub version_name: String,
    #[serde(default)]
    pub link: String,
}

#[derive(Deserialize)]
pub(crate) struct Builds {
    #[serde(default)]
    pub items: Vec<Build>,
}

impl Builds {
    /// The newest build of the default branch, as Galaxy picks it: GOG lists
    /// the newest first, older generations among them.
    pub fn current(self) -> Option<Build> {
        self.items
            .into_iter()
            .find(|build| build.branch.is_none())
            .filter(|build| build.generation == 2)
    }
}

/// A build's manifest.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BuildManifest {
    pub base_product_id: String,
    #[serde(default)]
    pub build_id: String,
    /// The folder Galaxy installs into. A Mac build may name its application
    /// bundle itself.
    #[serde(default)]
    pub install_directory: String,
    #[serde(default)]
    pub depots: Vec<Depot>,
    /// The redistributables the build needs, such as `MSVC2017`.
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub products: Vec<Product>,
    /// The build's installer scripts run in the Install Script Interpreter.
    #[serde(default)]
    pub script_interpreter: bool,
}

impl BuildManifest {
    /// Where the build's files go inside the installation folder: inside its
    /// bundle when the build names one, so the bundle keeps its `.app` name.
    pub fn content_root(&self) -> Result<String, GogError> {
        let directory = self.install_directory.replace('\\', "/");
        if !directory.to_ascii_lowercase().ends_with(".app") {
            return Ok(String::new());
        }
        safe_relative_path(&directory)?;
        if directory.contains('/') {
            return Err(GogError::Protocol);
        }
        Ok(directory)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Product {
    pub product_id: String,
    #[serde(default)]
    pub name: String,
    /// The installer a product without installer scripts runs, among its
    /// support files.
    #[serde(default)]
    pub temp_executable: String,
    #[serde(default)]
    pub temp_arguments: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Depot {
    pub product_id: String,
    pub manifest: String,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub size: u64,
}

impl Depot {
    /// Whether the depot belongs to an installation in `language`, such as
    /// `fr-FR`: a depot of every language, or of the language itself.
    pub fn speaks(&self, language: &str) -> bool {
        let base = |tag: &str| {
            tag.split(['-', '_'])
                .next()
                .unwrap_or(tag)
                .to_ascii_lowercase()
        };
        self.languages.iter().any(|tag| {
            tag == "*" || tag.eq_ignore_ascii_case(language) || base(tag) == base(language)
        })
    }

    pub fn universal(&self) -> bool {
        self.languages.iter().any(|tag| tag == "*")
    }
}

/// The depots an installation in `language` holds, of the products it owns:
/// GOG's own language, else English, as Galaxy falls back.
pub(crate) fn depots<'a>(
    manifest: &'a BuildManifest,
    products: &HashSet<&str>,
    language: &str,
) -> Vec<&'a Depot> {
    let owned: Vec<&Depot> = manifest
        .depots
        .iter()
        .filter(|depot| products.contains(depot.product_id.as_str()))
        .collect();
    let spoken = |language: &str| {
        owned
            .iter()
            .copied()
            .filter(|depot| depot.speaks(language))
            .collect::<Vec<_>>()
    };
    let localized = spoken(language);
    if localized.iter().any(|depot| !depot.universal()) || language.eq_ignore_ascii_case("en-US") {
        return localized;
    }
    spoken("en-US")
}

/// The redistributables' repository, which every build's dependencies name.
#[derive(Deserialize)]
pub(crate) struct DependencyRepository {
    #[serde(default)]
    pub depots: Vec<DependencyDepot>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DependencyDepot {
    pub dependency_id: String,
    pub manifest: String,
    #[serde(default)]
    pub executable: DependencyExecutable,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct DependencyExecutable {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub arguments: String,
}

impl DependencyDepot {
    /// The .NET Framework and XNA installers do not work in Wine: .NET games
    /// run on the Wine Mono Voile carries. Neither are Windows Installer
    /// packages run, nor installed.
    pub fn installs_in_wine(&self) -> bool {
        let id = self.dependency_id.to_ascii_lowercase();
        let framework = id.starts_with("dotnet")
            && !id.starts_with("dotnetcore")
            && !id.starts_with("dotnetdesktopruntime");
        !framework
            && !id.starts_with("xna")
            && !self.executable.path.to_ascii_lowercase().ends_with(".msi")
    }
}

/// A depot's manifest.
#[derive(Deserialize)]
pub(crate) struct DepotManifest {
    pub depot: DepotItems,
}

#[derive(Deserialize)]
pub(crate) struct DepotItems {
    #[serde(default)]
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum Item {
    DepotFile(File),
    DepotDirectory(Directory),
    DepotLink(Link),
    /// Kinds this client does not know are refused at validation.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Directory {
    pub path: String,
}

/// A link, whose target GOG gives from the installation's root.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Link {
    pub path: String,
    pub target: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct File {
    pub path: String,
    #[serde(default)]
    pub chunks: Vec<Chunk>,
    #[serde(default)]
    pub flags: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Chunk {
    pub md5: String,
    pub size: u64,
    pub compressed_md5: String,
    pub compressed_size: u64,
}

impl File {
    pub fn size(&self) -> u64 {
        self.chunks.iter().map(|chunk| chunk.size).sum()
    }
    pub fn executable(&self) -> bool {
        self.flags.iter().any(|flag| flag == "executable")
    }
    fn support(&self) -> bool {
        self.flags.iter().any(|flag| flag == "support")
    }
}

/// Where the content system keeps the document or chunk `hash` names: under
/// `ab/cd/abcdef…`. A manifest may already name that path.
pub(crate) fn galaxy_path(hash: &str) -> Result<String, GogError> {
    if hash.contains('/') {
        let parts: Vec<_> = hash.split('/').collect();
        let well_formed = parts.len() == 3
            && parts[0].len() == 2
            && parts[1].len() == 2
            && is_md5(parts[2])
            && parts[2].starts_with(&format!("{}{}", parts[0], parts[1]));
        return well_formed
            .then(|| hash.to_owned())
            .ok_or(GogError::Protocol);
    }
    if !is_md5(hash) {
        return Err(GogError::Protocol);
    }
    Ok(format!("{}/{}/{hash}", &hash[..2], &hash[2..4]))
}

pub(crate) fn is_md5(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Which secure link serves a file's chunks.
pub(crate) const DEPENDENCIES: &str = "dependencies";

/// Every file an installation holds, in manifest order, with where its chunks
/// come from. Directories and links follow.
#[derive(Debug, Default)]
pub(crate) struct Installed {
    pub files: Vec<InstalledFile>,
    pub directories: Vec<String>,
    pub links: Vec<Link>,
    /// Each file's place in `files`, by its lowercased path.
    positions: HashMap<String, usize>,
}

#[derive(Clone, Debug)]
pub(crate) struct InstalledFile {
    /// The file's place in the installation folder.
    pub path: String,
    pub file: File,
    /// The product whose secure link serves its chunks, or `DEPENDENCIES`.
    pub source: String,
}

impl Installed {
    /// Adds a depot's items under `root`, a folder inside the installation;
    /// a later depot's file replaces an earlier one of the same path, as
    /// Galaxy installs them in order. `source` serves their chunks. A
    /// product's support files go to its own support folder.
    pub fn add(&mut self, root: &str, source: &str, items: Vec<Item>) -> Result<(), GogError> {
        let place = |path: &str| -> Result<String, GogError> {
            let path = normalized(path)?;
            Ok(match root {
                "" => path,
                root => format!("{root}/{path}"),
            })
        };
        for item in items {
            match item {
                Item::DepotFile(file) => {
                    let path = match file.support() {
                        // Only a product's files have support files.
                        true if source == DEPENDENCIES => continue,
                        true => format!("{SUPPORT_DIRECTORY}/{source}/{}", normalized(&file.path)?),
                        false => place(&file.path)?,
                    };
                    let installed = InstalledFile {
                        path,
                        file,
                        source: source.to_owned(),
                    };
                    match self.positions.entry(installed.path.to_lowercase()) {
                        Entry::Occupied(kept) => self.files[*kept.get()] = installed,
                        Entry::Vacant(place) => {
                            place.insert(self.files.len());
                            self.files.push(installed);
                        }
                    }
                }
                Item::DepotDirectory(directory) => self.directories.push(place(&directory.path)?),
                Item::DepotLink(link) => self.links.push(Link {
                    path: place(&link.path)?,
                    target: place(&link.target)?,
                }),
                Item::Unknown => return Err(GogError::Protocol),
            }
        }
        Ok(())
    }

    /// Refuses what no installation may hold: an unsafe or duplicate path, a
    /// file inside another file, a malformed chunk, or a link leaving the
    /// installation.
    pub fn validate(&self) -> Result<(), GogError> {
        let mut paths = HashSet::new();
        for installed in &self.files {
            safe_relative_path(&installed.path)?;
            if !paths.insert(installed.path.to_lowercase()) {
                return Err(GogError::Protocol);
            }
            for chunk in &installed.file.chunks {
                if !is_md5(&chunk.md5)
                    || !is_md5(&chunk.compressed_md5)
                    || chunk.size == 0
                    || chunk.size > MAX_CHUNK_BYTES
                    || chunk.compressed_size == 0
                    || chunk.compressed_size > MAX_CHUNK_BYTES * 2
                {
                    return Err(GogError::Protocol);
                }
            }
        }
        // No file may sit inside another file, or behind a link.
        let links: HashSet<String> = self
            .links
            .iter()
            .map(|link| link.path.to_lowercase())
            .collect();
        for installed in &self.files {
            let path = safe_relative_path(&installed.path)?;
            for parent in path.ancestors().skip(1) {
                let parent = parent.to_string_lossy().to_lowercase();
                if !parent.is_empty() && (paths.contains(&parent) || links.contains(&parent)) {
                    return Err(GogError::Protocol);
                }
            }
        }
        for directory in &self.directories {
            safe_relative_path(directory)?;
        }
        for link in &self.links {
            safe_relative_path(&link.path)?;
            safe_relative_path(&link.target)
                .map_err(|_| GogError::UnsafePath(link.path.clone()))?;
            if paths.contains(&link.path.to_lowercase()) {
                return Err(GogError::UnsafePath(link.path.clone()));
            }
        }
        Ok(())
    }

    pub fn total_bytes(&self) -> u64 {
        self.files
            .iter()
            .map(|installed| installed.file.size())
            .sum()
    }
}

/// A manifest path with forward slashes and no leading separator: GOG's
/// depots spell them either way, whatever their platform.
fn normalized(path: &str) -> Result<String, GogError> {
    let path = path.replace('\\', "/");
    let path = path.trim_start_matches('/');
    if path.is_empty() {
        return Err(GogError::Protocol);
    }
    Ok(path.to_owned())
}

/// The relative target a link at `path` needs to reach `target`, both from
/// the installation's root.
pub(crate) fn relative_target(path: &str, target: &str) -> String {
    let depth = path.split('/').count().saturating_sub(1);
    let mut relative = "../".repeat(depth);
    relative.push_str(target);
    relative
}

#[cfg(test)]
mod tests;
