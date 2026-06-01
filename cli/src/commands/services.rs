use colored::Colorize;

// No /service/* routes exist on the server yet.
// All commands below are stubs with explicit TODOs.

// TODO: route POST /service not yet implemented on server
pub async fn create(name: &str, service_type: &str) -> Result<(), String> {
    // TODO: route POST /service not yet implemented on server
    println!("{}", "This command is not yet available".yellow());
    let _ = (name, service_type);
    Ok(())
}

// TODO: route GET /service not yet implemented on server
pub async fn list() -> Result<(), String> {
    // TODO: route GET /service not yet implemented on server
    println!("{}", "This command is not yet available".yellow());
    Ok(())
}

// TODO: route DELETE /service/{name} not yet implemented on server
pub async fn delete(name: &str) -> Result<(), String> {
    // TODO: route DELETE /service/{name} not yet implemented on server
    println!("{}", "This command is not yet available".yellow());
    let _ = name;
    Ok(())
}

// TODO: route POST /service/{name}/attach not yet implemented on server
pub async fn attach(name: &str, app: &str) -> Result<(), String> {
    // TODO: route POST /service/{name}/attach not yet implemented on server
    println!("{}", "This command is not yet available".yellow());
    let _ = (name, app);
    Ok(())
}

// TODO: route GET /service/{name} not yet implemented on server
pub async fn info(name: &str) -> Result<(), String> {
    // TODO: route GET /service/{name} not yet implemented on server
    println!("{}", "This command is not yet available".yellow());
    let _ = name;
    Ok(())
}
