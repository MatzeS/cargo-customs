use std::collections::HashMap;
use std::{path::Path, process::Stdio};

use cargo_metadata::Metadata;
use cargo_metadata::{MetadataCommand, Package};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::process::ExitCode;

use clap::Parser;

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("No 'Customs.toml' found.")]
    CustomsMissing,

    #[error("No regulations defined in '{0}'.")]
    NoRegulations(String),

    #[error("Invalid Customs file: {0}")]
    InvalidToml(#[from] toml::de::Error),

    #[error("Error from cargo: {0}")]
    Cargo(String),

    #[error("Unexpected I/O Error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Unexpected error: {0}")]
    Unexpected(#[from] anyhow::Error),
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Parser)]
struct Cli {
    #[clap(flatten)]
    manifest: clap_cargo::Manifest,
    #[clap(flatten)]
    workspace: clap_cargo::Workspace,
    #[clap(flatten)]
    features: clap_cargo::Features,
}

fn parse_cli() -> Cli {
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

    Cli::parse_from(args)
}

fn main() -> ExitCode {
    env_logger::init();
    let result = run();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log::error!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let args = parse_cli();

    let metadata = MetadataCommand::new().exec().map_err(|e| match e {
        cargo_metadata::Error::CargoMetadata { stderr } => Error::Cargo(stderr),
        _ => Error::Unexpected(e.into()),
    })?;

    let (packages_to_check, _) = args.workspace.partition_packages(&metadata);

    for package in packages_to_check.iter() {
        let info = load_customs(package, &metadata)?;

        let info = match info {
            Some(e) => e,
            None => {
                // If customs was invoked to target a single package,
                // then the user intent is to run a non-empty set of regulations.
                // Hence, not finding a customs definition is probably user error.
                if packages_to_check.len() == 1 && !args.workspace.workspace {
                    return Err(Error::CustomsMissing);
                } else {
                    // If there are multiple packages, it is plausible
                    // that not all have customs definitions, so a warning is sufficient.
                    log::warn!("No customs file for {}", package.manifest_path);
                    continue;
                }
            }
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

        if info.regulation.is_empty() {
            return Err(Error::NoRegulations(
                package.manifest_path.as_str().to_string(),
            ));
        }
    }

    Ok(())
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
#[serde(rename_all = "kebab-case")]
pub struct Regulation {
    // TODO strongly type the strings
    #[serde(default)]
    pub platform_targets: Vec<String>,

    #[serde(default)]
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
        return Ok(None);
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
        command.arg(self.job.name.as_str());

        // Not sure how to work around this specialization.
        // Either there needs an opt out/in to the target matrix conceptt
        // or an abstraction over the target matrix.
        if self.job.name != "fmt" {
            command
                .arg(format!("--target={platform_target}"))
                .arg(build_target);
        }

        command
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
