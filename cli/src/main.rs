mod commands;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{ArgValueCandidates, CompleteEnv, Shell};
use commands::{apps, complete, resources};

#[derive(Parser)]
#[command(
    name = "paastech",
    about = "PaasTech CLI — deploy and manage your apps",
    version,
    propagate_version = true
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
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
    /// Generate shell completion script
    Completion {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Subcommand)]
enum AppCommands {
    /// Deploy an application from a Docker image
    Deploy {
        /// Application name
        name: String,
        /// Docker image to deploy (e.g. nginx, node:20)
        #[arg(long)]
        image: String,
        /// Port the container exposes (1-65535)
        #[arg(long, default_value = "8080", value_parser = clap::value_parser!(u16).range(1..))]
        port: u16,
    },
    /// List all applications
    List,
    /// Delete an application
    Delete {
        /// Application name
        #[arg(add = ArgValueCandidates::new(complete::app_names))]
        name: String,
    },
    /// Show info about an application
    Info {
        /// Application name
        #[arg(add = ArgValueCandidates::new(complete::app_names))]
        name: String,
    },
    /// Stop a running application
    Stop {
        /// Application name
        #[arg(add = ArgValueCandidates::new(complete::app_names))]
        name: String,
    },
    /// Restart an application
    Restart {
        /// Application name
        #[arg(add = ArgValueCandidates::new(complete::app_names))]
        name: String,
    },
    /// Upload a zip of source code to the server
    Upload {
        /// Path to the zip file
        #[arg(long, value_hint = clap::ValueHint::FilePath)]
        source: String,
    },
    /// Show logs for an application
    Logs {
        /// Application name
        #[arg(add = ArgValueCandidates::new(complete::app_names))]
        name: String,
        /// Number of lines to show from the end
        #[arg(long)]
        tail: Option<u32>,
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
        #[arg(add = ArgValueCandidates::new(complete::app_names))]
        name: String,
        /// Key=Value pair
        pair: String,
    },
    /// List environment variables for an application
    List {
        /// Application name
        #[arg(add = ArgValueCandidates::new(complete::app_names))]
        name: String,
    },
}

#[derive(Subcommand)]
enum ResourceCommands {
    /// Create a managed resource
    #[command(disable_version_flag = true)]
    Create {
        /// Resource display name
        name: String,
        /// Resource type
        #[arg(long, value_parser = ["postgres", "redis", "s3"])]
        r#type: String,
        /// Docker Hub version tag
        #[arg(long)]
        version: String,
        /// Application name to link at creation (repeatable)
        #[arg(long, add = ArgValueCandidates::new(complete::app_names))]
        link: Vec<String>,
    },
    /// List all resources
    List,
    /// Show info about a resource
    Info {
        /// Resource name
        #[arg(add = ArgValueCandidates::new(complete::resource_names))]
        name: String,
    },
    /// Delete a resource
    Delete {
        /// Resource name
        #[arg(add = ArgValueCandidates::new(complete::resource_names))]
        name: String,
    },
    /// Update a resource's version or linked application
    #[command(disable_version_flag = true)]
    Edit {
        /// Resource name
        #[arg(add = ArgValueCandidates::new(complete::resource_names))]
        name: String,
        /// New Docker Hub version tag
        #[arg(long)]
        version: Option<String>,
        /// Application name to link (repeatable)
        #[arg(long, add = ArgValueCandidates::new(complete::app_names))]
        link: Vec<String>,
    },
    /// Attach a resource to one or more applications
    Attach {
        /// Resource name
        #[arg(add = ArgValueCandidates::new(complete::resource_names))]
        name: String,
        /// Application name (repeatable)
        #[arg(long, add = ArgValueCandidates::new(complete::app_names))]
        app: Vec<String>,
    },
    /// Start a stopped resource
    Start {
        /// Resource name
        #[arg(add = ArgValueCandidates::new(complete::resource_names))]
        name: String,
    },
    /// Stop a running resource
    Stop {
        /// Resource name
        #[arg(add = ArgValueCandidates::new(complete::resource_names))]
        name: String,
    },
    /// Show logs for a resource
    Logs {
        /// Resource name
        #[arg(add = ArgValueCandidates::new(complete::resource_names))]
        name: String,
        /// Number of lines to show from the end
        #[arg(long)]
        tail: Option<u32>,
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
        #[arg(add = ArgValueCandidates::new(complete::resource_names))]
        name: String,
        /// Key=Value pair
        pair: String,
    },
    /// List environment variables for a resource
    List {
        /// Resource name
        #[arg(add = ArgValueCandidates::new(complete::resource_names))]
        name: String,
    },
}

pub fn api_base() -> String {
    std::env::var("PAAS_API_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".to_string())
}

fn main() {
    // Intercepts tab completion requests (COMPLETE=<shell> env var) before starting the runtime.
    // This allows ArgValueCandidates callbacks to create their own tokio runtime safely.
    CompleteEnv::with_factory(Cli::command).complete();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run());
}

async fn run() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Completion { shell } => {
            // Output the completion registration script for the requested shell.
            // Setting COMPLETE=<shell> makes CompleteEnv write the script and exit.
            let shell_str = match shell {
                Shell::Bash => "bash",
                Shell::Zsh => "zsh",
                Shell::Fish => "fish",
                Shell::Elvish => "elvish",
                Shell::PowerShell => "powershell",
                _ => return,
            };
            unsafe { std::env::set_var("COMPLETE", shell_str) };
            CompleteEnv::with_factory(Cli::command).complete();
            return;
        }
        Commands::App { command } => match command {
            AppCommands::Deploy { name, image, port } => apps::deploy(&name, &image, port).await,
            AppCommands::List => apps::list().await,
            AppCommands::Delete { name } => apps::delete(&name).await,
            AppCommands::Info { name } => apps::info(&name).await,
            AppCommands::Stop { name } => apps::stop(&name).await,
            AppCommands::Restart { name } => apps::restart(&name).await,
            AppCommands::Upload { source } => apps::upload(&source).await,
            AppCommands::Logs { name, tail } => apps::logs(&name, tail).await,
            AppCommands::Env { command } => match command {
                EnvCommands::Set { name, pair } => apps::env_set(&name, &pair).await,
                EnvCommands::List { name } => apps::env_list(&name).await,
            },
        },
        Commands::Resource { command } => match command {
            ResourceCommands::Create {
                name,
                r#type,
                version,
                link,
            } => {
                let refs: Vec<&str> = link.iter().map(|s| s.as_str()).collect();
                resources::create(&name, &r#type, &version, &refs).await
            }
            ResourceCommands::List => resources::list().await,
            ResourceCommands::Info { name } => resources::info(&name).await,
            ResourceCommands::Delete { name } => resources::delete(&name).await,
            ResourceCommands::Edit {
                name,
                version,
                link,
            } => {
                let refs: Vec<&str> = link.iter().map(|s| s.as_str()).collect();
                resources::edit(&name, version.as_deref(), &refs).await
            }
            ResourceCommands::Attach { name, app } => {
                let refs: Vec<&str> = app.iter().map(|s| s.as_str()).collect();
                resources::attach(&name, &refs).await
            }
            ResourceCommands::Start { name } => resources::start(&name).await,
            ResourceCommands::Stop { name } => resources::stop(&name).await,
            ResourceCommands::Logs { name, tail } => resources::logs(&name, tail).await,
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
