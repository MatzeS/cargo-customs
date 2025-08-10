use std::collections::HashMap;
use std::path::PathBuf;
use std::{path::Path, process::Stdio};

use anyhow::anyhow;
use cargo_metadata::Metadata;
use cargo_metadata::{MetadataCommand, Package};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::process::ExitCode;

use clap::Parser;

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("No 'Customs.toml' found for '{0}'.")]
    CustomsMissing(PathBuf),

    #[error("Invalid Customs file: {0}")]
    InvalidToml(#[from] toml::de::Error),

    #[error("Unexpected I/O Error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Unexpected error: `{0}`")]
    Unexpected(#[from] anyhow::Error),
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Parser)]
struct CommandlineArguments {
    // TODO run in local crate only
    // TODO add workspace flag to run for the workspace
    // TODO check clap options
    #[arg(long)]
    workspace: bool,
}

fn parse_cli() -> CommandlineArguments {
    const CARGO_COMMAND_NAME: &str = "customs";

    let mut args = std::env::args().peekable();
    let executable = args
        .next()
        .expect("exec must be invoked with at least the executable");
    if let Some(first_arg) = args.peek()
        && first_arg == CARGO_COMMAND_NAME
    {
        // cargo may invoke with the subcommand in the first place,
        // in this case it is simply discarded.
        let _customs = args.next();
    }

    let args = std::iter::once(executable).chain(args);

    CommandlineArguments::parse_from(args)
}

fn main() -> ExitCode {
    let result = run();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let args = parse_cli();

    // If customs  is invoked with workspace
    // then run on all workspace members

    // otherwise run on the local package
    // if the local package is a workspace, run on workspace

    let metadata = MetadataCommand::new().exec().map_err(|e| anyhow!(e))?;

    let cwd = std::env::current_dir().map_err(|e| anyhow!(e))?;

    let current_package = find_current_package(&metadata, &cwd);

    let packages_to_check: Vec<&Package> = if args.workspace {
        metadata.workspace_packages()
    } else {
        match current_package {
            Some(e) => std::iter::once(e).collect(),

            // If current_package is None, no clearly identified through cwd,
            // then we run on the entire workspace.
            None => metadata.workspace_packages(),
        }
    };

    for package in packages_to_check {
        let info = load_customs(package, &metadata)?;
        let info = match info {
            Some(e) => e,
            None => continue,
        };

        let directory = package
            .manifest_path
            .parent()
            .expect("Manifest must be in some directory");

        // TODO Flatten all customs first to regulations
        // sort regulations
        for regulation in info.regulation.iter().flat_map(|e| e.expand()) {
            regulation.check(directory.as_std_path())?;
        }
    }

    Ok(())
}

/// Finds the package that standard cargo execution would be targeting.
///
/// Essentially, this finds the next manifest up the file tree from the given cwd.
///
/// Instead of walking the file system, this implementation finds the corresponding manifest my identifying the manifest,
/// which shares the longest prefix with the cwd.
///
/// TODO check how this works with nested cargo crates, specifically examples
/// and if the standard cargo behavior is documented (/implemented) somewhere
///
/// TODO this fails if you are in an intermediate directory between workspace root and package
fn find_current_package<'a>(metadata: &'a Metadata, cwd: &Path) -> Option<&'a Package> {
    let cwd = cwd.to_str().expect("Failed path to string conversion");

    metadata
        .packages
        .iter()
        .filter(|e| metadata.workspace_members.contains(&e.id))
        .map(|p| {
            let package_dir = p
                .manifest_path
                .parent()
                .expect("every manifest must be in a directory");
            let package_dir = package_dir.as_str();
            if cwd.starts_with(package_dir) {
                (p, package_dir.len())
            } else {
                (p, 0)
            }
        })
        .max_by(|(_, a), (_, b)| a.cmp(b))
        .filter(|(_, e)| *e > 0)
        .map(|(p, _)| p)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomsFile {
    pub default: Option<Regulation>,

    #[serde(default)]
    pub regulation: Vec<Regulation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Regulation {
    // TODO strongly type the strings
    // TODO check rename case style
    #[serde(default)]
    #[serde(rename = "platform-targets")]
    pub platform_targets: Vec<String>,

    #[serde(default)]
    #[serde(rename = "build-targets")]
    pub build_targets: Vec<String>,

    #[serde(default)]
    pub jobs: Jobs,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Jobs {
    Short(Vec<String>),
    Detailed(HashMap<String, JobParameters>),
}

impl Default for Jobs {
    fn default() -> Self {
        Jobs::Short(Vec::new())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobParameters {
    #[serde(default)]
    args: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Job {
    name: String,
    args: Vec<String>,
}

impl Job {
    fn from_short(name: String) -> Job {
        Self {
            name,
            args: Vec::new(),
        }
    }

    fn from_parameters(name: String, parameters: JobParameters) -> Job {
        Self {
            name,
            args: parameters.args,
        }
    }
}

impl Jobs {
    fn into_jobs(self) -> Vec<Job> {
        match self {
            Jobs::Short(items) => items.into_iter().map(Job::from_short).collect(),
            Jobs::Detailed(map) => map
                .into_iter()
                .map(|(name, parameters)| Job::from_parameters(name, parameters))
                .collect(),
        }
    }
}

fn read_customs_file(path: &Path) -> Result<CustomsFile> {
    let data = std::fs::read_to_string(path)?;
    Ok(toml::from_str(data.as_str())?)
}

fn load_customs(package: &Package, metadata: &Metadata) -> Result<Option<CustomsFile>> {
    const CUSTOMS_FILE_NAME: &str = "Customs.toml";

    let workspace_root = metadata.workspace_root.clone();

    let crate_customs_path = package
        .manifest_path
        .parent()
        .expect("manifest must be in directory")
        .join(CUSTOMS_FILE_NAME);

    if !std::fs::exists(crate_customs_path.as_std_path())? {
        return Err(Error::CustomsMissing(
            crate_customs_path.into_std_path_buf(),
        ));
    }

    let mut _crate_customs = read_customs_file(crate_customs_path.as_std_path())?;

    // TODO must be not empty

    // Take all Customs.toml files from the directories between the package Cargo and the workspace cargo
    let customs = package
        .manifest_path
        .ancestors()
        .take_while(|e| e.as_std_path() != workspace_root.as_std_path())
        // Safety: because the iterator is below the workspace root,
        // there is at least the workspace root as a parent.
        .map(|e| e.parent().unwrap().join(CUSTOMS_FILE_NAME))
        .collect::<Vec<_>>();

    // Read all applicable Customs files
    let customs = customs
        .into_iter()
        .map(std::fs::read_to_string)
        .flat_map(|e| e.ok())
        .map(|e| -> Result<CustomsFile> { Ok(toml::from_str(&e)?) })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .rev()
        .collect::<Vec<_>>();

    // TODO this is not clean, considering there could be no Customs.toml in a crate.
    // TODO further, last does not point to the correct file if there is no customs file in the crate
    let mut crate_customs = customs.last().unwrap().clone();

    let default = customs
        .into_iter()
        .flat_map(|e| e.default.clone()) // TODO this clone seems unnecessary
        .last();

    // fill any empty sets with defaults
    if let Some(default) = default {
        for regulation in crate_customs.regulation.iter_mut() {
            if regulation.platform_targets.is_empty() {
                regulation.platform_targets = default.platform_targets.clone();
            }

            if regulation.build_targets.is_empty() {
                regulation.build_targets = default.build_targets.clone();
            }

            // TODO this cloen is rathehr inefficient
            if regulation.jobs.clone().into_jobs().is_empty() {
                regulation.jobs = default.jobs.clone();
            }
        }
    }

    // TODO print warning on empty sets

    Ok(Some(crate_customs))
}

impl Regulation {
    pub fn expand(&self) -> Vec<RegulationCheck> {
        let build_targets = self.build_targets.clone();
        const ALL_BUILD_TARGETS_DESIGNATOR: &str = "all";
        if build_targets
            .iter()
            .any(|e| e == ALL_BUILD_TARGETS_DESIGNATOR)
            && build_targets.len() != 1
        {
            panic!("build-targets all can only be alone");
        }

        // TODO inefficient clone
        let jobs = self.jobs.clone().into_jobs();
        self.platform_targets
            .iter()
            .cartesian_product(build_targets.iter())
            .cartesian_product(jobs.iter())
            .map(|((p, b), j)| RegulationCheck {
                platform_target: p.clone(),
                build_target: b.clone(),
                job: j.clone(),
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct RegulationCheck {
    pub platform_target: String,
    pub build_target: String,
    pub job: Job,
}

impl RegulationCheck {
    pub fn check(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let build_target: String = match self.build_target.as_str() {
            "lib" => "--lib".into(),
            "bins" => "--bins".into(),
            "tests" => "--tests".into(),
            // TODO bench
            // TODO examples
            "all" => "--all-targets".into(),
            _ => {
                // TODO add similarly for tests bench exampmles
                if let Some(bin) = self.build_target.strip_prefix("bin:") {
                    format!("--bin={bin}")
                } else {
                    panic!("invalid build target {}", self.build_target)
                }
            }
        };
        let mut platform_target = self.platform_target.clone();
        const HOST_PLATFORM_DESIGNATOR: &str = "host";
        if platform_target == HOST_PLATFORM_DESIGNATOR {
            platform_target = get_host_platform_target();
        }

        let mut command = std::process::Command::new("cargo");
        command
            .arg(self.job.name.as_str())
            .arg(format!("--target={platform_target}"))
            .arg(build_target)
            .current_dir(path)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        if !self.job.args.is_empty() {
            command.arg("--");
        }
        for arg in self.job.args.iter() {
            command.arg(arg.as_str());
        }

        let status = command.status()?;

        if !status.success() {
            anyhow::bail!("failed"); // TODO proper error logging
        }

        Ok(())
    }
}

fn get_host_platform_target() -> String {
    use rustc_version::version_meta;
    let meta = version_meta().expect("Failed to get rustc version");
    meta.host
}
