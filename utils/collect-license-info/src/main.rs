use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use askama::Template;
use cargo_license::{DependencyDetails, GetDependenciesOpt};
use cargo_metadata::{CargoOpt, MetadataCommand, Package};
use gumdrop::Options;
use spdx::{Expression, LicenseId};

/// Collects license information for the Nitrokey 3 firmware and its dependencies and prints a
/// document with the license details to stdout.
#[derive(Debug, Options)]
struct Args {
    /// Show this help message.
    help: bool,
    /// The path of the Cargo manifest to use.
    #[options(free)]
    manifest: PathBuf,
    #[options(free)]
    product: String,
}

impl Args {
    fn firmware(&self) -> Dependency {
        let metadata = MetadataCommand::from(self)
            .exec()
            .expect("failed to query firmware metadata");
        metadata
            .root_package()
            .expect("missing root package")
            .to_owned()
            .into()
    }

    fn dependencies(&self) -> BTreeSet<Dependency> {
        cargo_license::get_dependencies_from_cargo_lock(
            self.into(),
            GetDependenciesOpt {
                avoid_dev_deps: false,
                avoid_build_deps: false,
                direct_deps_only: false,
                avoid_proc_macros: false,
                root_only: false,
            },
        )
        .expect("failed to collect dependencies")
        .into_iter()
        .filter(|d| d.license.is_some())
        .map(Dependency::from)
        .collect()
    }
}

impl From<&Args> for MetadataCommand {
    fn from(args: &Args) -> Self {
        let mut cmd = MetadataCommand::new();
        cmd.features(CargoOpt::AllFeatures);
        cmd.manifest_path(&args.manifest);
        cmd
    }
}

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Dependency {
    name: String,
    authors: Vec<String>,
    license_expression: String,
    licenses: Vec<LicenseId>,
}

impl From<DependencyDetails> for Dependency {
    fn from(d: DependencyDetails) -> Self {
        let license_expression = d.license.expect("missing license");
        let licenses = Expression::parse(&license_expression)
            .expect("failed to parse license expression")
            .requirements()
            .map(|e| e.req.license.id().expect("missing license ID"))
            .collect();
        Self {
            name: d.name,
            authors: d
                .authors
                .unwrap_or_default()
                .split('|')
                .map(ToOwned::to_owned)
                .collect(),
            license_expression,
            licenses,
        }
    }
}

impl From<Package> for Dependency {
    fn from(p: Package) -> Self {
        let license_expression = p.license.expect("missing license");
        let licenses = Expression::parse(&license_expression)
            .expect("failed to parse license expression")
            .requirements()
            .map(|e| e.req.license.id().expect("missing license ID"))
            .collect();
        Self {
            name: p.name,
            authors: p.authors,
            license_expression,
            licenses,
        }
    }
}

#[derive(Debug, Template)]
#[template(path = "license.txt")]
struct LicenseTemplate<'a> {
    product: &'a str,
    firmware: &'a Dependency,
    dependencies: &'a BTreeSet<Dependency>,
    licenses: &'a BTreeMap<LicenseId, &'a str>,
}

fn main() {
    let args = Args::parse_args_default_or_exit();
    let firmware = args.firmware();
    let dependencies = args.dependencies();

    let mut licenses: BTreeSet<_> = dependencies.iter().flat_map(|d| &d.licenses).collect();
    licenses.extend(&firmware.licenses);
    let licenses: BTreeMap<_, _> = licenses
        .into_iter()
        .map(|license| (*license, license.text()))
        .collect();
    let template = LicenseTemplate {
        product: &args.product,
        firmware: &firmware,
        dependencies: &dependencies,
        licenses: &licenses,
    };

    println!(
        "{}",
        template.render().expect("failed to render license.txt")
    );
}
