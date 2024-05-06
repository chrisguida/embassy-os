use std::collections::BTreeMap;

use clap::Parser;
use emver::VersionRange;
use itertools::Itertools;
use rpc_toolkit::{from_fn_async, Context, HandlerExt, ParentHandler};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::context::CliContext;
use crate::prelude::*;
use crate::registry::context::RegistryContext;
use crate::registry::os::index::OsVersionInfo;
use crate::registry::signer::SignerKey;
use crate::util::serde::{display_serializable, HandlerExtSerde, WithIoFormat};
use crate::util::Version;

pub mod signer;

pub fn version_api<C: Context>() -> ParentHandler<C> {
    ParentHandler::new()
        .subcommand(
            "add",
            from_fn_async(add_version)
                .with_metadata("admin", Value::Bool(true))
                .with_metadata("getSigner", Value::Bool(true))
                .no_display()
                .with_call_remote::<CliContext>(),
        )
        .subcommand(
            "remove",
            from_fn_async(remove_version)
                .with_metadata("admin", Value::Bool(true))
                .no_display()
                .with_call_remote::<CliContext>(),
        )
        .subcommand("signer", signer::signer_api::<C>())
        .subcommand(
            "get",
            from_fn_async(get_version)
                .with_display_serializable()
                .with_custom_display_fn(|handle, result| {
                    Ok(display_version_info(handle.params, result))
                })
                .with_call_remote::<CliContext>(),
        )
}

#[derive(Debug, Deserialize, Serialize, Parser, TS)]
#[command(rename_all = "kebab-case")]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AddVersionParams {
    #[ts(type = "string")]
    pub version: Version,
    pub headline: String,
    pub release_notes: String,
    #[ts(type = "string")]
    pub source_version: VersionRange,
    #[arg(skip)]
    #[ts(skip)]
    #[serde(rename = "__auth_signer")]
    pub signer: Option<SignerKey>,
}

pub async fn add_version(
    ctx: RegistryContext,
    AddVersionParams {
        version,
        headline,
        release_notes,
        source_version,
        signer,
    }: AddVersionParams,
) -> Result<(), Error> {
    ctx.db
        .mutate(|db| {
            let signer = signer
                .map(|s| db.as_index().as_signers().get_signer(&s))
                .transpose()?;
            db.as_index_mut()
                .as_os_mut()
                .as_versions_mut()
                .upsert(&version, || OsVersionInfo::default())?
                .mutate(|i| {
                    i.headline = headline;
                    i.release_notes = release_notes;
                    i.source_version = source_version;
                    i.signers.extend(signer);
                    Ok(())
                })
        })
        .await
}

#[derive(Debug, Deserialize, Serialize, Parser, TS)]
#[command(rename_all = "kebab-case")]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoveVersionParams {
    #[ts(type = "string")]
    pub version: Version,
}

pub async fn remove_version(
    ctx: RegistryContext,
    RemoveVersionParams { version }: RemoveVersionParams,
) -> Result<(), Error> {
    ctx.db
        .mutate(|db| {
            db.as_index_mut()
                .as_os_mut()
                .as_versions_mut()
                .remove(&version)?;
            Ok(())
        })
        .await
}

#[derive(Debug, Deserialize, Serialize, Parser, TS)]
#[command(rename_all = "kebab-case")]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GetVersionParams {
    #[ts(type = "string | null")]
    #[arg(long = "src")]
    pub source: Option<Version>,
    #[ts(type = "string | null")]
    #[arg(long = "target")]
    pub target: Option<VersionRange>,
}

pub async fn get_version(
    ctx: RegistryContext,
    GetVersionParams { source, target }: GetVersionParams,
) -> Result<BTreeMap<Version, OsVersionInfo>, Error> {
    let target = target.unwrap_or(VersionRange::Any);
    ctx.db
        .peek()
        .await
        .into_index()
        .into_os()
        .into_versions()
        .into_entries()?
        .into_iter()
        .map(|(v, i)| i.de().map(|i| (v, i)))
        .filter_ok(|(version, info)| {
            version.satisfies(&target)
                && source
                    .as_ref()
                    .map_or(true, |s| s.satisfies(&info.source_version))
        })
        .collect()
}

pub fn display_version_info<T>(params: WithIoFormat<T>, info: BTreeMap<Version, OsVersionInfo>) {
    use prettytable::*;

    if let Some(format) = params.format {
        return display_serializable(format, info);
    }

    let mut table = Table::new();
    table.add_row(row![bc =>
        "VERSION",
        "HEADLINE",
        "RELEASE NOTES",
        "ISO PLATFORMS",
        "IMG PLATFORMS",
        "SQUASHFS PLATFORMS",
    ]);
    for (version, info) in &info {
        table.add_row(row![
            version.as_str(),
            &info.headline,
            &info.release_notes,
            &info.iso.keys().into_iter().join(", "),
            &info.img.keys().into_iter().join(", "),
            &info.squashfs.keys().into_iter().join(", "),
        ]);
    }
    table.print_tty(false).unwrap();
}