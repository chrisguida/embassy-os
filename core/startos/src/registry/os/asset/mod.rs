use rpc_toolkit::{from_fn_async, Context, HandlerExt, ParentHandler};

pub mod add;
pub mod get;
pub mod sign;

pub fn asset_api<C: Context>() -> ParentHandler<C> {
    ParentHandler::new()
        .subcommand("add", add::add_api::<C>())
        .subcommand("add", from_fn_async(add::cli_add_asset).no_display())
        .subcommand("sign", sign::sign_api::<C>())
        .subcommand("sign", from_fn_async(sign::cli_sign_asset).no_display())
        .subcommand("get", get::get_api::<C>())
}