use clap::Parser;
use color_eyre::eyre::eyre;
use models::PackageId;
use serde::{Deserialize, Serialize};
use tracing::instrument;
use ts_rs::TS;

use crate::context::RpcContext;
use crate::prelude::*;
use crate::Error;

#[derive(Deserialize, Serialize, Parser, TS)]
#[serde(rename_all = "camelCase")]
#[command(rename_all = "kebab-case")]
pub struct ControlParams {
    pub id: PackageId,
}

#[instrument(skip_all)]
pub async fn start(ctx: RpcContext, ControlParams { id }: ControlParams) -> Result<(), Error> {
    ctx.services
        .get(&id)
        .await
        .as_ref()
        .or_not_found(lazy_format!("Manager for {id}"))?
        .start()
        .await?;

    Ok(())
}

pub async fn stop(ctx: RpcContext, ControlParams { id }: ControlParams) -> Result<(), Error> {
    // TODO: why did this return last_status before?
    ctx.services
        .get(&id)
        .await
        .as_ref()
        .ok_or_else(|| Error::new(eyre!("Manager not found"), crate::ErrorKind::InvalidRequest))?
        .stop()
        .await?;

    Ok(())
}

pub async fn restart(ctx: RpcContext, ControlParams { id }: ControlParams) -> Result<(), Error> {
    ctx.services
        .get(&id)
        .await
        .as_ref()
        .ok_or_else(|| Error::new(eyre!("Manager not found"), crate::ErrorKind::InvalidRequest))?
        .restart()
        .await?;

    Ok(())
}