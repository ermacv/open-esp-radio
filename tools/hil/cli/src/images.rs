//! `cargo hil images`: the HIL image classes checked by hand, and compared
//! between revisions.
//!
//! - `check`: build or type-check image classes with their link-time audits
//!   ([`check`]), every class, the named ones or those a change reaches;
//! - `compare`: the classes built at a base revision and in this checkout,
//!   function by function modulo placement.

pub mod check;

use oer_hil_model::schema::image::ImageClass;
use oer_os_process::Checkout;

use crate::Result;

#[derive(clap::Parser)]
#[command(name = "images", about = "Check and compare the HIL image classes")]
pub enum ImagesCli {
    /// Build HIL image classes with their link-time audits, reporting every
    /// class: all of them with `--all`, each `--class`, or the classes a
    /// change reaches (`--reaching`); `--list` prints every class
    /// with the runtime features it builds with.
    #[command(group(clap::ArgGroup::new("selection").required(true).args(["all", "classes", "list", "reaching", "instrumented"])))]
    Check {
        #[arg(long)]
        all: bool,
        #[arg(long)]
        list: bool,
        /// Only classes whose runtime declares instrumentation.
        #[arg(long)]
        instrumented: bool,
        #[arg(long = "class")]
        classes: Vec<ImageClass>,
        /// Packages with changed chip code: every class whose image compiles
        /// one of them.
        #[arg(long, value_delimiter = ',')]
        reaching: Vec<String>,
        /// Fail first when the lock gives an image a vendor dependency.
        #[arg(long)]
        vendor_dependencies_absent: bool,
        /// Only `cargo check` each runtime, without code generation or audits.
        #[arg(long)]
        type_check: bool,
        /// Run Clippy with the image build's features and compiler configuration.
        #[arg(long, conflicts_with = "type_check")]
        clippy: bool,
        /// Classes built at once; defaults to half the cores, at most 8.
        #[arg(long)]
        jobs: Option<usize>,
        /// Only this chip's classes; without it, every chip whose HIL agent
        /// builds a selected class, one after the other.
        #[arg(long)]
        chip: Option<String>,
    },
    /// HIL image classes built at BASE and in this checkout, compared
    /// function by function modulo placement.
    Compare {
        #[arg(long)]
        base: String,
        #[arg(long = "class", default_values_t = [String::from("boot-smoke")])]
        classes: Vec<String>,
        /// Reviewed rename applied to both images, FROM=TO.
        #[arg(long = "alias")]
        aliases: Vec<String>,
        /// Reviewed function whose code may differ.
        #[arg(long = "allow")]
        allowed: Vec<String>,
        #[arg(long)]
        show: Vec<String>,
        /// The chip to build for; without it, the one chip whose HIL agent
        /// builds every class.
        #[arg(long)]
        chip: Option<String>,
    },
}

pub fn command(ctx: &Checkout, cli: ImagesCli) -> Result<std::process::ExitCode> {
    match cli {
        ImagesCli::Check { list: true, .. } => print!("{}", check::list()),
        ImagesCli::Check {
            all,
            classes,
            reaching,
            vendor_dependencies_absent,
            type_check,
            clippy,
            instrumented,
            jobs,
            chip,
            ..
        } => {
            if vendor_dependencies_absent {
                oer_hil_run::image::ensure_vendor_dependencies_absent(&ctx.root)?;
            }
            let depth = if type_check {
                check::Depth::Check(oer_image_build::RuntimeCheck::Types)
            } else if clippy {
                check::Depth::Check(oer_image_build::RuntimeCheck::Clippy)
            } else {
                check::Depth::Build
            };
            // Only every chip's whole catalog, built, shows which coverage
            // reviews, shared between chips, no image applies.
            let catalog = all && chip.is_none() && depth == check::Depth::Build;
            let mut built = Vec::new();
            for chip in check::chips(&ctx.root, chip.as_deref(), &classes)? {
                let classes = if instrumented {
                    let classes = check::instrumented(&ctx.root, &chip)?;
                    if classes.is_empty() {
                        continue;
                    }
                    classes
                } else if reaching.is_empty() {
                    classes.clone()
                } else {
                    check::reached(&ctx.root, &chip, &reaching)?
                };
                let outcomes = check::run(
                    ctx,
                    &chip,
                    &classes,
                    depth,
                    jobs.unwrap_or_else(check::default_jobs),
                )?;
                built.push((chip, outcomes));
            }
            if catalog {
                check::stale_coverage(&ctx.root, &built)?;
            }
        }
        ImagesCli::Compare {
            base,
            classes,
            aliases,
            allowed,
            show,
            chip,
        } => {
            let aliases = aliases
                .into_iter()
                .map(|alias| {
                    alias
                        .split_once('=')
                        .map(|(a, b)| (a.to_owned(), b.to_owned()))
                        .ok_or_else(|| format!("alias `{alias}` is not FROM=TO").into())
                })
                .collect::<Result<Vec<_>>>()?;
            let review = oer_image_compare::Review {
                aliases: oer_image_compare::Aliases(aliases),
                allowed: allowed.into_iter().collect(),
                show,
            };
            let parsed = classes
                .iter()
                .map(|class| class.parse())
                .collect::<std::result::Result<Vec<ImageClass>, _>>()?;
            let chip = oer_hil_run::image::chip_for(&ctx.root, &parsed, chip.as_deref())?;
            oer_hil_run::image::compare_images(&ctx.root, &chip, &base, &parsed, &review)?;
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// `cargo hil observer`: build the runner without running HIL, and print
/// its private copy's path.
pub fn observer(ctx: &Checkout) -> Result<std::process::ExitCode> {
    println!("{}", oer_hil_observer::prepare::build(&ctx.root)?.display());
    Ok(std::process::ExitCode::SUCCESS)
}
