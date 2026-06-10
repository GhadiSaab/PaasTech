use clap_complete::engine::{CompletionCandidate, ValueCompleter};
use serde::Deserialize;
use std::ffi::OsStr;
use std::sync::OnceLock;
use std::time::Duration;

static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();

fn get_client() -> &'static reqwest::blocking::Client {
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new())
    })
}

pub(crate) fn fetch_app_names_from(api_base: &str) -> Vec<String> {
    #[derive(Deserialize)]
    struct App {
        name: String,
    }

    let base = api_base.trim_end_matches('/');
    let url = match crate::commands::projects::current_project() {
        Ok(Some(project)) => format!("{}/project/{}/app", base, project),
        _ => format!("{}/app", base),
    };
    get_client()
        .get(url)
        .send()
        .ok()
        .and_then(|r| r.json::<Vec<App>>().ok())
        .map(|apps| apps.into_iter().map(|a| a.name).collect())
        .unwrap_or_default()
}

pub(crate) fn fetch_resource_names_from(api_base: &str) -> Vec<String> {
    #[derive(Deserialize)]
    struct Resource {
        display_name: String,
    }

    let base = api_base.trim_end_matches('/');
    let url = match crate::commands::projects::current_project() {
        Ok(Some(project)) => format!("{}/project/{}/resource", base, project),
        _ => format!("{}/resource", base),
    };
    get_client()
        .get(url)
        .send()
        .ok()
        .and_then(|r| r.json::<Vec<Resource>>().ok())
        .map(|resources| resources.into_iter().map(|r| r.display_name).collect())
        .unwrap_or_default()
}

pub(crate) fn complete_names(names: Vec<String>, current: &OsStr) -> Vec<CompletionCandidate> {
    let prefix = current.to_string_lossy();
    names
        .into_iter()
        .filter(|n| n.starts_with(prefix.as_ref()))
        .map(CompletionCandidate::new)
        .collect()
}

pub struct AppNameCompleter;

impl ValueCompleter for AppNameCompleter {
    fn complete(&self, current: &OsStr) -> Vec<CompletionCandidate> {
        complete_names(fetch_app_names_from(&crate::api_base()), current)
    }
}

pub struct ResourceNameCompleter;

impl ValueCompleter for ResourceNameCompleter {
    fn complete(&self, current: &OsStr) -> Vec<CompletionCandidate> {
        complete_names(fetch_resource_names_from(&crate::api_base()), current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(candidates: Vec<CompletionCandidate>) -> Vec<String> {
        candidates
            .iter()
            .map(|c| c.get_value().to_string_lossy().to_string())
            .collect()
    }

    // --- Tests purs (sans HTTP) ---

    #[test]
    fn complete_names_empty_prefix_returns_all() {
        let names = vec!["alpha".into(), "beta".into(), "gamma".into()];
        assert_eq!(complete_names(names, OsStr::new("")).len(), 3);
    }

    #[test]
    fn complete_names_filters_by_prefix() {
        let names = vec!["prod-api".into(), "prod-web".into(), "staging".into()];
        let got = values(complete_names(names, OsStr::new("prod-")));
        assert_eq!(got, vec!["prod-api", "prod-web"]);
    }

    #[test]
    fn complete_names_exact_match_includes_extensions() {
        let names = vec!["my-app".into(), "my-app-v2".into(), "other".into()];
        let got = values(complete_names(names, OsStr::new("my-app")));
        assert_eq!(got, vec!["my-app", "my-app-v2"]);
    }

    #[test]
    fn complete_names_no_match_returns_empty() {
        let names = vec!["app-1".into()];
        assert!(complete_names(names, OsStr::new("xyz")).is_empty());
    }

    // --- Tests avec serveur HTTP fictif ---

    #[test]
    fn fetch_app_names_parses_response() {
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/app")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"[{"name":"api","status":"running"},{"name":"web","status":"stopped"}]"#)
            .create();

        let names = fetch_app_names_from(&server.url());
        assert_eq!(names, vec!["api", "web"]);
    }

    #[test]
    fn fetch_app_names_returns_empty_on_server_error() {
        let mut server = mockito::Server::new();
        server.mock("GET", "/app").with_status(500).create();

        assert!(fetch_app_names_from(&server.url()).is_empty());
    }

    #[test]
    fn fetch_app_names_returns_empty_on_connection_failure() {
        assert!(fetch_app_names_from("http://127.0.0.1:1").is_empty());
    }

    #[test]
    fn fetch_resource_names_parses_response() {
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/resource")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"[{"id":"uuid","display_name":"my-db","name":"postgres","version":"14","status":"running","application_ids":[]}]"#,
            )
            .create();

        let names = fetch_resource_names_from(&server.url());
        assert_eq!(names, vec!["my-db"]);
    }

    #[test]
    fn fetch_resource_names_empty_list() {
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/resource")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("[]")
            .create();

        assert!(fetch_resource_names_from(&server.url()).is_empty());
    }

    #[test]
    fn fetch_resource_names_returns_empty_on_server_error() {
        let mut server = mockito::Server::new();
        server.mock("GET", "/resource").with_status(500).create();

        assert!(fetch_resource_names_from(&server.url()).is_empty());
    }

    #[test]
    fn app_name_completer_filters_by_prefix() {
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/app")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"[{"name":"prod-api"},{"name":"prod-web"},{"name":"staging"}]"#)
            .create();

        // Appelle fetch_app_names_from directement pour tester le filtrage intégré
        let names = fetch_app_names_from(&server.url());
        let got = values(complete_names(names, OsStr::new("prod-")));
        assert_eq!(got, vec!["prod-api", "prod-web"]);
    }
}
