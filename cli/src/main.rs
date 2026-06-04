mod commands;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use commands::{apps, resources};
use std::io;

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
    Service {
        #[command(subcommand)]
        command: ServiceCommands,
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
        name: String,
    },
    /// Show info about an application
    Info {
        /// Application name
        name: String,
    },
    /// Stop a running application
    Stop {
        /// Application name
        name: String,
    },
    /// Restart an application
    Restart {
        /// Application name
        name: String,
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
        name: String,
        /// Key=Value pair
        pair: String,
    },
    /// List environment variables for an application
    List {
        /// Application name
        name: String,
    },
}

#[derive(Subcommand)]
enum ServiceCommands {
    /// Create a managed service
    Create {
        /// Service name
        name: String,
        /// Service type
        #[arg(long, value_parser = ["postgres", "redis", "s3"])]
        r#type: String,
    },
    /// List all resources
    List,
    /// Delete a service
    Delete {
        /// Service name
        name: String,
    },
    /// Attach a service to an application
    Attach {
        /// Service name
        name: String,
        /// Application to attach to
        #[arg(long)]
        app: String,
    },
    /// Show info about a service
    Info {
        /// Service name
        name: String,
    },
}

pub fn api_base() -> String {
    std::env::var("PAAS_API_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".to_string())
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Completion { shell } => {
            generate(shell, &mut Cli::command(), "paastech", &mut io::stdout());
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
        Commands::Service { command } => match command {
            ServiceCommands::Create { name, r#type } => resources::create(&name, &r#type).await,
            ServiceCommands::List => resources::list().await,
            ServiceCommands::Delete { name } => resources::delete(&name).await,
            ServiceCommands::Attach { name, app } => resources::attach(&name, &app).await,
            ServiceCommands::Info { name } => resources::info(&name).await,
        },
    };

    if let Err(e) = result {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}
