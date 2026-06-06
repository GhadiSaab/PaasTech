use actix_multipart::Multipart;
use actix_web::{Error, web};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::process::Command as TokioCommand;

pub struct MultipartData {
    pub file_path: Option<PathBuf>,
    pub fields: HashMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProcessType {
    Web,
    Worker,
}

impl ProcessType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::Worker => "worker",
        }
    }
}

impl TryFrom<String> for ProcessType {
    type Error = String;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        match s.as_str() {
            "web" => Ok(Self::Web),
            "worker" => Ok(Self::Worker),
            other => Err(format!("unknown process type: {other}")),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProcessDefinition {
    pub name: String,
    #[serde(rename = "type")]
    pub process_type: ProcessType,
    pub path: String,
    pub port: Option<u16>,
    pub public_host: Option<String>,
    #[serde(default)]
    pub build_env: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct PaastechManifest {
    #[serde(default)]
    processes: Vec<ProcessDefinition>,
}

pub async fn save_multipart_file(mut payload: Multipart) -> Result<MultipartData, Error> {
    let mut file_path = None;
    let mut fields = HashMap::new();

    while let Ok(Some(mut field)) = payload.try_next().await {
        let content_disposition = match field.content_disposition() {
            Some(cd) => cd,
            None => continue,
        };

        let field_name = content_disposition.get_name().unwrap_or("").to_string();

        if let Some(filename) = content_disposition.get_filename() {
            let filename = filename.to_string();
            let filepath = PathBuf::from(format!("/tmp/uploads/{}", filename));
            let mut file = File::create(&filepath).await?;
            while let Ok(Some(chunk)) = field.try_next().await {
                file.write_all(&chunk).await?;
            }
            file.flush().await?;
            file_path = Some(filepath);
        } else {
            let mut value = String::new();
            while let Ok(Some(chunk)) = field.try_next().await {
                value.push_str(&String::from_utf8_lossy(&chunk));
            }
            fields.insert(field_name, value);
        }
    }

    Ok(MultipartData { file_path, fields })
}

pub fn load_process_definitions(
    root: &Path,
    fallback_port: Option<u16>,
) -> Result<Vec<ProcessDefinition>, String> {
    let manifest_path = root.join("paastech.toml");

    if !manifest_path.is_file() {
        let fallback = ProcessDefinition {
            name: "web".to_string(),
            process_type: ProcessType::Web,
            path: ".".to_string(),
            port: fallback_port,
            public_host: None,
            build_env: HashMap::new(),
        };
        validate_process_definition(root, &fallback)?;
        return Ok(vec![fallback]);
    }

    let manifest = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("Failed to read {}: {e}", manifest_path.display()))?;
    let manifest: PaastechManifest =
        toml::from_str(&manifest).map_err(|e| format!("Invalid paastech.toml: {e}"))?;

    if manifest.processes.is_empty() {
        let fallback = ProcessDefinition {
            name: "web".to_string(),
            process_type: ProcessType::Web,
            path: ".".to_string(),
            port: fallback_port,
            public_host: None,
            build_env: HashMap::new(),
        };
        validate_process_definition(root, &fallback)?;
        return Ok(vec![fallback]);
    }

    let mut seen_names = std::collections::HashSet::new();
    for process in &manifest.processes {
        if !seen_names.insert(process.name.as_str()) {
            return Err(format!(
                "paastech.toml contains duplicate process name '{}'",
                process.name
            ));
        }
        validate_process_definition(root, process)?;
    }

    Ok(manifest.processes)
}

fn validate_process_definition(root: &Path, process: &ProcessDefinition) -> Result<(), String> {
    if process.name.trim().is_empty() {
        return Err("process name cannot be empty".to_string());
    }

    if process
        .name
        .chars()
        .any(|c| !(c.is_ascii_alphanumeric() || c == '-'))
    {
        return Err(format!(
            "process '{}' must only contain letters, numbers, and hyphens",
            process.name
        ));
    }

    let path = Path::new(&process.path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "process '{}' path must stay inside the uploaded archive",
            process.name
        ));
    }

    let context = root.join(path);
    if !context.is_dir() {
        return Err(format!(
            "process '{}' build context does not exist: {}",
            process.name,
            context.display()
        ));
    }

    if process.process_type == ProcessType::Web && process.port.is_none() {
        return Err(format!(
            "web process '{}' must declare a port in paastech.toml",
            process.name
        ));
    }

    if let Some(public_host) = process.public_host.as_deref() {
        validate_public_host(&process.name, public_host)?;
    }

    for (key, value) in &process.build_env {
        validate_env_key(&process.name, key)?;
        if value.contains('\n') || value.contains('\0') {
            return Err(format!(
                "process '{}' build_env key '{}' value must not contain newlines or null bytes",
                process.name, key
            ));
        }
    }

    Ok(())
}

fn validate_public_host(process_name: &str, public_host: &str) -> Result<(), String> {
    if public_host.trim().is_empty()
        || public_host.contains('/')
        || public_host.contains(':')
        || public_host.chars().any(char::is_whitespace)
    {
        return Err(format!(
            "process '{}' public_host must be a hostname without scheme, port, slash, or whitespace",
            process_name
        ));
    }

    Ok(())
}

fn validate_env_key(process_name: &str, key: &str) -> Result<(), String> {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return Err(format!(
            "process '{}' build_env has an empty key",
            process_name
        ));
    };

    if !(first.is_ascii_alphabetic() || first == '_')
        || chars.any(|c| !(c.is_ascii_alphanumeric() || c == '_'))
    {
        return Err(format!(
            "process '{}' build_env key '{}' is not a valid environment variable name",
            process_name, key
        ));
    }

    Ok(())
}

pub async fn extract_zip(source: PathBuf) -> Result<PathBuf, String> {
    let mut dest_path = source.clone();
    if let Some(stem) = source.file_stem() {
        dest_path.set_file_name(format!("{}-extract", stem.to_string_lossy()));
    } else {
        dest_path.push("-extract");
    }
    let dest_for_closure = dest_path.clone();

    web::block(move || {
        let sync_file = std::fs::File::open(&source)
            .map_err(|e| format!("Failed to open file for extraction: {}", e))?;

        zip_extract::extract(sync_file, &dest_for_closure, true)
            .map_err(|e| format!("Zip extraction error: {}", e))
    })
    .await
    .map_err(|e| format!("Thread pool error: {}", e))?
    .map(|_| dest_path)
}

pub async fn build_image_with_name(
    image_name: &str,
    from: String,
    docker_host: &str,
    build_env: &HashMap<String, String>,
) -> Result<(), String> {
    let builder = std::env::var("BUILDER").map_err(|_| "BUILDER env var is not set".to_string())?;

    let mut cmd = TokioCommand::new("pack");
    cmd.args(["build", image_name, "--path", &from, "--builder", &builder]);

    let mut build_env: Vec<_> = build_env.iter().collect();
    build_env.sort_by_key(|(key, _)| *key);
    for (key, value) in build_env {
        cmd.args(["--env", &format!("{key}={value}")]);
    }

    if !docker_host.is_empty() {
        cmd.args(["--docker-host", docker_host]);
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| format!("Failed to run pack build: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "Build failed with exit code {}:\n--- stderr ---\n{}\n--- stdout ---\n{}",
            output.status, stderr, stdout
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_project() -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("paastech-engine-test-{suffix}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn project_only_manifest_falls_back_to_web_process() {
        let root = temp_project();
        fs::write(root.join("paastech.toml"), "[project]\nname = \"demo\"\n").unwrap();

        let processes = load_process_definitions(&root, Some(8080)).unwrap();

        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].name, "web");
        assert_eq!(processes[0].process_type, ProcessType::Web);
        assert_eq!(processes[0].path, ".");
        assert_eq!(processes[0].port, Some(8080));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manifest_can_include_project_metadata_and_processes() {
        let root = temp_project();
        fs::create_dir_all(root.join("api")).unwrap();
        fs::write(
            root.join("paastech.toml"),
            r#"
[project]
name = "demo"

[[processes]]
name = "api"
type = "web"
path = "api"
port = 3000
"#,
        )
        .unwrap();

        let processes = load_process_definitions(&root, None).unwrap();

        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].name, "api");
        assert_eq!(processes[0].process_type, ProcessType::Web);
        assert_eq!(processes[0].path, "api");
        assert_eq!(processes[0].port, Some(3000));

        fs::remove_dir_all(root).unwrap();
    }
}
