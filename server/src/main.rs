#![forbid(unsafe_code)]
#![forbid(non_ascii_idents)]
#![allow(clippy::nonstandard_macro_braces)]

use std::time::Duration;

use crate::{
    domain::{
        handler::{CreateUserRequest, GroupBackendHandler, GroupRequestFilter, UserBackendHandler},
        sql_backend_handler::SqlBackendHandler,
        sql_opaque_handler::register_password,
    },
    infra::{cli::*, configuration::Configuration, db_cleaner::Scheduler, healthcheck, mail},
};
use actix::Actor;
use actix_server::ServerBuilder;
use anyhow::{anyhow, Context, Result};
use futures_util::TryFutureExt;
use sea_orm::Database;
use tracing::*;

mod domain;
mod infra;

async fn create_admin_user(handler: &SqlBackendHandler, config: &Configuration) -> Result<()> {
    let pass_length = config.ldap_user_pass.unsecure().len();
    assert!(
        pass_length >= 8,
        "Minimum password length is 8 characters, got {} characters",
        pass_length
    );
    handler
        .create_user(CreateUserRequest {
            user_id: config.ldap_user_dn.clone(),
            email: config.ldap_user_email.clone(),
            display_name: Some("Administrator".to_string()),
            ..Default::default()
        })
        .and_then(|_| register_password(handler, &config.ldap_user_dn, &config.ldap_user_pass))
        .await
        .context("Error creating admin user")?;
    let groups = handler
        .list_groups(Some(GroupRequestFilter::DisplayName(
            "lldap_admin".to_owned(),
        )))
        .await?;
    assert_eq!(groups.len(), 1);
    handler
        .add_user_to_group(&config.ldap_user_dn, groups[0].id)
        .await
        .context("Error adding admin user to group")
}

async fn ensure_group_exists(handler: &SqlBackendHandler, group_name: &str) -> Result<()> {
    if handler
        .list_groups(Some(GroupRequestFilter::DisplayName(group_name.to_owned())))
        .await?
        .is_empty()
    {
        warn!("Could not find {} group, trying to create it", group_name);
        handler
            .create_group(group_name)
            .await
            .context(format!("while creating {} group", group_name))?;
    }
    Ok(())
}

#[instrument(skip_all)]
async fn set_up_server(config: Configuration) -> Result<ServerBuilder> {
    info!("Starting LLDAP version {}", env!("CARGO_PKG_VERSION"));

    let sql_pool = {
        let mut sql_opt = sea_orm::ConnectOptions::new(config.database_url.clone());
        sql_opt
            .max_connections(5)
            .sqlx_logging(true)
            .sqlx_logging_level(log::LevelFilter::Debug);
        Database::connect(sql_opt).await?
    };
    domain::sql_tables::init_table(&sql_pool)
        .await
        .context("while creating the tables")?;
    let backend_handler = SqlBackendHandler::new(config.clone(), sql_pool.clone());
    ensure_group_exists(&backend_handler, "lldap_admin").await?;
    ensure_group_exists(&backend_handler, "lldap_password_manager").await?;
    ensure_group_exists(&backend_handler, "lldap_strict_readonly").await?;
    if let Err(e) = backend_handler.get_user_details(&config.ldap_user_dn).await {
        warn!("Could not get admin user, trying to create it: {:#}", e);
        create_admin_user(&backend_handler, &config)
            .await
            .map_err(|e| anyhow!("Error setting up admin login/account: {:#}", e))
            .context("while creating the admin user")?;
    }
    let server_builder = infra::ldap_server::build_ldap_server(
        &config,
        backend_handler.clone(),
        actix_server::Server::build(),
    )
    .context("while binding the LDAP server")?;
    infra::jwt_sql_tables::init_table(&sql_pool).await?;
    let server_builder =
        infra::tcp_server::build_tcp_server(&config, backend_handler, server_builder)
            .await
            .context("while binding the TCP server")?;
    // Run every hour.
    let scheduler = Scheduler::new("0 0 * * * * *", sql_pool);
    scheduler.start();
    Ok(server_builder)
}

async fn run_server(config: Configuration) -> Result<()> {
    set_up_server(config)
        .await?
        .workers(1)
        .run()
        .await
        .context("while starting the server")?;
    Ok(())
}

fn run_server_command(opts: RunOpts) -> Result<()> {
    debug!("CLI: {:#?}", &opts);

    let config = infra::configuration::init(opts)?;
    infra::logging::init(&config)?;

    actix::run(
        run_server(config).unwrap_or_else(|e| error!("Could not bring up the servers: {:#}", e)),
    )?;

    info!("End.");
    Ok(())
}

fn send_test_email_command(opts: TestEmailOpts) -> Result<()> {
    let to = opts.to.parse()?;
    let config = infra::configuration::init(opts)?;
    infra::logging::init(&config)?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    runtime.block_on(
        mail::send_test_email(to, &config.smtp_options)
            .unwrap_or_else(|e| error!("Could not send email: {:#}", e)),
    );
    Ok(())
}

fn run_healthcheck(opts: RunOpts) -> Result<()> {
    debug!("CLI: {:#?}", &opts);
    let config = infra::configuration::init(opts)?;
    infra::logging::init(&config)?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    use tokio::time::timeout;
    let delay = Duration::from_millis(3000);
    let (ldap, ldaps, api) = runtime.block_on(async {
        tokio::join!(
            timeout(delay, healthcheck::check_ldap(config.ldap_port)),
            timeout(delay, healthcheck::check_ldaps(&config.ldaps_options)),
            timeout(delay, healthcheck::check_api(config.http_port)),
        )
    });

    let mut failure = false;
    [ldap, ldaps, api]
        .into_iter()
        .filter_map(Result::err)
        .for_each(|e| {
            failure = true;
            error!("{:#}", e)
        });
    std::process::exit(i32::from(failure))
}

fn main() -> Result<()> {
    let cli_opts = infra::cli::init();
    match cli_opts.command {
        Command::ExportGraphQLSchema(opts) => infra::graphql::api::export_schema(opts),
        Command::Run(opts) => run_server_command(opts),
        Command::HealthCheck(opts) => run_healthcheck(opts),
        Command::SendTestEmail(opts) => send_test_email_command(opts),
    }
}
