//! `cargo stand router`: the lab router's airtime queue limits (mac80211's
//! AQL) through the LAB service, under the caller's lease or a lease of
//! their own. A set is journaled by the service, and the router's end state
//! after the lease restores mac80211's defaults.

use oer_tools_lab_arbiter::{
    claims::{AIR, Claim},
    readiness::ROUTER,
};
use oer_tools_lab_client::protocol::{Operation, RouterAccessCategory, RouterOperation};

use crate::Result;

#[derive(clap::Parser)]
#[command(name = "cargo stand router", no_binary_name = true)]
pub(crate) struct RouterCli {
    #[command(subcommand)]
    command: RouterCommand,
}

#[derive(clap::Subcommand)]
enum RouterCommand {
    /// Print the AP radio's airtime limits per access category and its
    /// total threshold as JSON, and with `--station` that station's own
    /// limits and pending airtime.
    Airtime {
        #[arg(long, value_name = "MAC")]
        station: Option<String>,
    },
    /// Set one access category's airtime limits on the AP radio and print
    /// what the router reads back as JSON. Needs the router exclusively;
    /// the lease's end returns every category to mac80211's defaults.
    SetAirtime {
        #[arg(long, value_enum)]
        ac: Category,
        #[arg(long, value_name = "MICROSECONDS")]
        low_us: u32,
        #[arg(long, value_name = "MICROSECONDS")]
        high_us: u32,
    },
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum Category {
    Vo,
    Vi,
    Be,
    Bk,
}

pub(crate) fn router(owner: String, cli: RouterCli) -> Result<std::process::ExitCode> {
    let (request, router) = match cli.command {
        RouterCommand::Airtime { station } => (
            RouterOperation::AirtimeLimits { mac: station },
            Claim::shared(ROUTER),
        ),
        RouterCommand::SetAirtime {
            ac,
            low_us,
            high_us,
        } => {
            if !inside_lease() {
                return Err(
                    "a set airtime limit lasts until its lease ends, and a lease of its \
                            own would end at once: run it inside `cargo stand lease --stand -- …`"
                        .into(),
                );
            }
            (
                RouterOperation::SetAirtimeLimit {
                    ac: match ac {
                        Category::Vo => RouterAccessCategory::Vo,
                        Category::Vi => RouterAccessCategory::Vi,
                        Category::Be => RouterAccessCategory::Be,
                        Category::Bk => RouterAccessCategory::Bk,
                    },
                    low_us,
                    high_us,
                },
                Claim::exclusive(ROUTER),
            )
        }
    };
    // Every router operation needs the air too; the service refuses one
    // without it.
    let mut client = crate::service::lease(
        owner,
        String::from("stand router airtime"),
        vec![router, Claim::shared(AIR)],
    )?;
    let answer = crate::service::run(&mut client, Operation::Router { request })?;
    println!("{}", serde_json::to_string_pretty(&answer)?);
    client.release()?;
    Ok(std::process::ExitCode::SUCCESS)
}

/// Whether this command runs inside a lease it joins.
fn inside_lease() -> bool {
    std::env::var(oer_tools_lab_client::protocol::LEASE_ENV).is_ok_and(|token| !token.is_empty())
}
