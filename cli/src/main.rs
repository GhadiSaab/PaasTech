mod commands;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::engine::ArgValueCompleter;
use clap_complete::{CompleteEnv, Shell, generate};
use commands::completions::{AppNameCompleter, ResourceNameCompleter};
use commands::{apps, projects, resources};
use std::io;

#[derive(Parser)]
#[command(
    name = "paastech",
    about = "PaasTech CLI — deploy and manage your apps",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a project in the current directory
    Init {
        /// Project name
        name: String,
    },
    /// Deploy the current project directory using its local manifest/buildpack files
    Deploy {
        /// Fallback port for projects without process declarations
        #[arg(long, default_value = "8080", value_parser = clap::value_parser!(u16).range(1..))]
        port: u16,
    },
    /// Redeploy the current project with zero downtime; requires a prior deployment
    Update {
        /// Fallback port for projects without process declarations
        #[arg(long, default_value = "8080", value_parser = clap::value_parser!(u16).range(1..))]
        port: u16,
    },
    /// Manage the active project
    Project {
        #[command(subcommand)]
        command: ProjectCommands,
    },
    /// Manage applications
    App {
        #[command(subcommand)]
        command: AppCommands,
    },
    /// Manage resources (postgres, redis, s3)
    Resource {
        #[command(subcommand)]
        command: ResourceCommands,
    },
    /// Generate shell completion script (static)
    Completion {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Subcommand)]
enum ProjectCommands {
    /// Show active project info
    Info,
    /// List projects
    List,
    /// Delete a project
    Delete {
        /// Project name
        name: String,
    },
    /// Manage project environment variables
    Env {
        #[command(subcommand)]
        command: ProjectEnvCommands,
    },
}

#[derive(Subcommand)]
enum ProjectEnvCommands {
    /// Set an environment variable (format: KEY=VALUE)
    Set {
        /// Key=Value pair
        pair: String,
    },
    /// List environment variables
    List,
}

#[derive(Subcommand)]
enum AppCommands {
    /// List all applications
    List,
    /// Delete an application
    Delete {
        /// Application name
        #[arg(add = ArgValueCompleter::new(AppNameCompleter))]
        name: String,
    },
    /// Show info about an application
    Info {
        /// Application name
        #[arg(add = ArgValueCompleter::new(AppNameCompleter))]
        name: String,
        /// Process name
        #[arg(long)]
        process: Option<String>,
    },
    /// Stop a running application
    Stop {
        /// Application name
        #[arg(add = ArgValueCompleter::new(AppNameCompleter))]
        name: String,
        /// Stop only this process
        #[arg(long)]
        process: Option<String>,
    },
    /// Restart an application
    Restart {
        /// Application name
        #[arg(add = ArgValueCompleter::new(AppNameCompleter))]
        name: String,
        /// Restart only this process
        #[arg(long)]
        process: Option<String>,
    },
    /// Upload a zip of source code to the server
    Upload {
        /// Path to the zip file
        #[arg(long)]
        source: String,
    },
    /// Show logs for an application
    Logs {
        /// Application name
        #[arg(add = ArgValueCompleter::new(AppNameCompleter))]
        name: String,
        /// Process name to read logs from
        #[arg(long)]
        process: Option<String>,
        /// Number of lines to show from the end
        #[arg(long)]
        tail: Option<u32>,
        /// Stream logs in real time (like docker logs -f)
        #[arg(long, short = 'f')]
        follow: bool,
    },
    /// Manage environment variables
    Env {
        #[command(subcommand)]
        command: EnvCommands,
    },
}

#[derive(Subcommand)]
enum EnvCommands {
    /// Set an environment variable (format: KEY=VALUE)
    Set {
        /// Application name
        #[arg(add = ArgValueCompleter::new(AppNameCompleter))]
        name: String,
        /// Process name
        #[arg(long, default_value = "web")]
        process: String,
        /// Key=Value pair
        pair: String,
    },
    /// List environment variables for an application
    List {
        /// Application name
        #[arg(add = ArgValueCompleter::new(AppNameCompleter))]
        name: String,
        /// Process name
        #[arg(long, default_value = "web")]
        process: String,
    },
}

#[derive(Subcommand)]
enum ResourceCommands {
    /// Create a managed resource
    Create {
        /// Resource display name
        name: String,
        /// Resource type
        #[arg(long, value_parser = ["postgres", "redis", "s3"])]
        r#type: String,
        /// Docker Hub version tag
        #[arg(long)]
        version: Option<String>,
        /// Application name to link at creation (repeatable)
        #[arg(long, add = ArgValueCompleter::new(AppNameCompleter))]
        link: Vec<String>,
        /// Connection profile to use for linked apps (for postgres: sync or asyncpg)
        #[arg(long)]
        connection: Option<String>,
    },
    /// List all resources
    List,
    /// Show info about a resource
    Info {
        /// Resource name
        #[arg(add = ArgValueCompleter::new(ResourceNameCompleter))]
        name: String,
    },
    /// Delete a resource
    Delete {
        /// Resource name
        #[arg(add = ArgValueCompleter::new(ResourceNameCompleter))]
        name: String,
    },
    /// Update a resource's version or linked application
    Edit {
        /// Resource name
        #[arg(add = ArgValueCompleter::new(ResourceNameCompleter))]
        name: String,
        /// New Docker Hub version tag
        #[arg(long)]
        version: Option<String>,
        /// Application name to link (repeatable)
        #[arg(long, add = ArgValueCompleter::new(AppNameCompleter))]
        link: Vec<String>,
        /// Connection profile to use for linked apps (for postgres: sync or asyncpg)
        #[arg(long)]
        connection: Option<String>,
    },
    /// Attach a resource to one or more applications
    Attach {
        /// Resource name
        #[arg(add = ArgValueCompleter::new(ResourceNameCompleter))]
        name: String,
        /// Application name (repeatable)
        #[arg(long, add = ArgValueCompleter::new(AppNameCompleter))]
        app: Vec<String>,
        /// Connection profile to use for attached apps (for postgres: sync or asyncpg)
        #[arg(long)]
        connection: Option<String>,
    },
    /// Start a stopped resource
    Start {
        /// Resource name
        #[arg(add = ArgValueCompleter::new(ResourceNameCompleter))]
        name: String,
    },
    /// Stop a running resource
    Stop {
        /// Resource name
        #[arg(add = ArgValueCompleter::new(ResourceNameCompleter))]
        name: String,
    },
    /// Show logs for a resource
    Logs {
        /// Resource name
        #[arg(add = ArgValueCompleter::new(ResourceNameCompleter))]
        name: String,
        /// Number of lines to show from the end
        #[arg(long)]
        tail: Option<u32>,
        /// Stream logs in real time (like docker logs -f)
        #[arg(long, short = 'f')]
        follow: bool,
    },
    /// List available versions for a service type (e.g. postgres, redis, s3)
    Versions {
        /// Service type name
        name: String,
    },
    /// Manage environment variables
    Env {
        #[command(subcommand)]
        command: ResourceEnvCommands,
    },
}

#[derive(Subcommand)]
enum ResourceEnvCommands {
    /// Set an environment variable (format: KEY=VALUE)
    Set {
        /// Resource name
        #[arg(add = ArgValueCompleter::new(ResourceNameCompleter))]
        name: String,
        /// Key=Value pair
        pair: String,
    },
    /// List environment variables for a resource
    List {
        /// Resource name
        #[arg(add = ArgValueCompleter::new(ResourceNameCompleter))]
        name: String,
    },
}

pub fn api_base() -> String {
    std::env::var("PAAS_API_URL").unwrap_or_else(|_| "https://api.paastech.izoret.fr".to_string())
}

fn main() {
    CompleteEnv::with_factory(Cli::command).complete();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime")
        .block_on(async_main());
}

async fn async_main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Init { name } => projects::init(&name).await,
        Commands::Deploy { port } => apps::deploy_current_dir(port).await,
        Commands::Update { port } => apps::update_current_dir(port).await,
        Commands::Project { command } => match command {
            ProjectCommands::Info => projects::info().await,
            ProjectCommands::List => projects::list().await,
            ProjectCommands::Delete { name } => projects::delete(&name).await,
            ProjectCommands::Env { command } => match command {
                ProjectEnvCommands::Set { pair } => projects::env_set(&pair).await,
                ProjectEnvCommands::List => projects::env_list().await,
            },
        },
        Commands::Completion { shell } => {
            generate(shell, &mut Cli::command(), "paastech", &mut io::stdout());
            return;
        }
        Commands::App { command } => match command {
            AppCommands::List => apps::list().await,
            AppCommands::Delete { name } => apps::delete(&name).await,
            AppCommands::Info { name, process } => apps::info(&name, process.as_deref()).await,
            AppCommands::Stop { name, process } => apps::stop(&name, process.as_deref()).await,
            AppCommands::Restart { name, process } => {
                apps::restart(&name, process.as_deref()).await
            }
            AppCommands::Upload { source } => apps::upload(&source).await,
            AppCommands::Logs {
                name,
                process,
                tail,
                follow,
            } => apps::logs(&name, tail, follow, process.as_deref()).await,
            AppCommands::Env { command } => match command {
                EnvCommands::Set {
                    name,
                    process,
                    pair,
                } => apps::env_set(&name, &process, &pair).await,
                EnvCommands::List { name, process } => apps::env_list(&name, &process).await,
            },
        },
        Commands::Resource { command } => match command {
            ResourceCommands::Create {
                name,
                r#type,
                version,
                link,
                connection,
            } => {
                let refs: Vec<&str> = link.iter().map(|s| s.as_str()).collect();
                resources::create(
                    &name,
                    &r#type,
                    version.as_deref(),
                    &refs,
                    connection.as_deref(),
                )
                .await
            }
            ResourceCommands::List => resources::list().await,
            ResourceCommands::Info { name } => resources::info(&name).await,
            ResourceCommands::Delete { name } => resources::delete(&name).await,
            ResourceCommands::Edit {
                name,
                version,
                link,
                connection,
            } => {
                let refs: Vec<&str> = link.iter().map(|s| s.as_str()).collect();
                resources::edit(&name, version.as_deref(), &refs, connection.as_deref()).await
            }
            ResourceCommands::Attach {
                name,
                app,
                connection,
            } => {
                let refs: Vec<&str> = app.iter().map(|s| s.as_str()).collect();
                resources::attach(&name, &refs, connection.as_deref()).await
            }
            ResourceCommands::Start { name } => resources::start(&name).await,
            ResourceCommands::Stop { name } => resources::stop(&name).await,
            ResourceCommands::Logs { name, tail, follow } => {
                resources::logs(&name, tail, follow).await
            }
            ResourceCommands::Versions { name } => resources::versions(&name).await,
            ResourceCommands::Env { command } => match command {
                ResourceEnvCommands::Set { name, pair } => resources::env_set(&name, &pair).await,
                ResourceEnvCommands::List { name } => resources::env_list(&name).await,
            },
        },
    };

    if let Err(e) = result {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}
