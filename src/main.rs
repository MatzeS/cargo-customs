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

        if info.regulation.is_empty() {
            return Err(Error::NoRegulations(
                package.manifest_path.as_str().to_string(),
            ));
        }

        // TODO Flatten all customs first to regulations
        // sort regulations
        for regulation in info.regulation.into_iter().flat_map(|e| e.expand()) {
            regulation.check(directory.as_std_path())?;
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

    #[serde(default = "default_feature_sets")]
    pub feature_sets: Vec<Vec<String>>,
}

fn default_feature_sets() -> Vec<Vec<String>> {
    vec![vec![]]
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

    fn is_empty(&self) -> bool {
        match self {
            Jobs::Short(items) => items.is_empty(),
            Jobs::Detailed(hash_map) => hash_map.is_empty(),
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

    let mut crate_customs = read_customs_file(crate_customs_path.as_std_path())?;

    // Take all Customs.toml upwards (and including) the current crate
    // to find the next applicable default
    let ancestor_customs = package
        .manifest_path
        .ancestors()
        .take_while(|e| e.as_std_path() != workspace_root.as_std_path())
        // Safety: because the iterator is below the workspace root,
        // there is at least the workspace root as a parent.
        .map(|e| e.parent().unwrap().join(CUSTOMS_FILE_NAME))
        .map(|e| read_customs_file(e.as_std_path()))
        .flat_map(|e| e.ok())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();

    let default = ancestor_customs.into_iter().flat_map(|e| e.default).last();

    // fill any empty sets with defaults
    if let Some(default) = default {
        for regulation in crate_customs.regulation.iter_mut() {
            if regulation.platform_targets.is_empty() {
                regulation.platform_targets = default.platform_targets.clone();
            }

            if regulation.build_targets.is_empty() {
                regulation.build_targets = default.build_targets.clone();
            }

            if regulation.jobs.is_empty() {
                regulation.jobs = default.jobs.clone();
            }
        }
    }

    Ok(Some(crate_customs))
}

impl Regulation {
    pub fn expand(self) -> Vec<RegulationCheck> {
        let build_targets = self.build_targets.clone();
        const ALL_BUILD_TARGETS_DESIGNATOR: &str = "all";
        if build_targets
            .iter()
            .any(|e| e == ALL_BUILD_TARGETS_DESIGNATOR)
            && build_targets.len() != 1
        {
            panic!("build-targets all can only be alone");
        }

        let jobs = self.jobs.into_jobs();
        let features = self.feature_sets.clone();
        self.platform_targets
            .iter()
            .cartesian_product(build_targets.iter())
            .cartesian_product(jobs.iter())
            .cartesian_product(features.iter())
            .map(|(((p, b), j), f)| RegulationCheck {
                platform_target: p.clone(),
                build_target: b.clone(),
                job: j.clone(),
                features: f.clone(),
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct RegulationCheck {
    pub platform_target: String,
    pub build_target: String,
    pub job: Job,
    pub features: Vec<String>,
}

fn convert_build_target_specifier_to_cargo_argument(input: &str) -> String {
    // lib is technically not a group of targets, but it is invoked without argument and hence handled here
    let groups = ["lib", "bins", "examples", "tests", "benches", "all-targets"];

    if groups.contains(&input) {
        return format!("--{input}");
    }

    if input == "all" {
        return "--all-targets".into();
    }

    let prefixes = ["bin:", "example:", "test:", "bench:"];
    for prefix in prefixes {
        if input.strip_prefix(prefix).is_some() {
            return format!("--{}", input.to_string().replace(":", "="));
        }
    }

    panic!("invalid build target {}", input)
}

impl RegulationCheck {
    pub fn check(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let build_target: String =
            convert_build_target_specifier_to_cargo_argument(self.build_target.as_str());

        let mut platform_target = Some(self.platform_target.as_str());
        const HOST_PLATFORM_DESIGNATOR: &str = "host";
        if platform_target.is_some_and(|e| e == HOST_PLATFORM_DESIGNATOR) {
            platform_target = None;
        }

        let mut command = std::process::Command::new("cargo");
        command.arg(self.job.name.as_str());

        // Not sure how to work around this specialization.
        // Either there needs an opt out/in to the target matrix conceptt
        // or an abstraction over the target matrix.
        if self.job.name != "fmt" {
            command.arg(build_target);
        }

        if let Some(platform_target) = platform_target {
            command.arg(format!("--target={platform_target}"));
        }

        if !self.features.is_empty() {
            command.arg("--features").arg(self.features.join(","));
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
