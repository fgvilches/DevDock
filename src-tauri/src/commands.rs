use crate::models::*;
use crate::process_manager::{self, ProcessManager};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command as StdCommand;
use std::sync::Arc;
use tauri::{AppHandle, State};
use walkdir::WalkDir;

/// Extra PATH dirs so launchd-launched apps find dev CLIs (node, cargo, go...).
pub fn path_dirs() -> Vec<String> {
    let mut v = vec![
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
        "/usr/bin".to_string(),
        "/bin".to_string(),
        "/usr/sbin".to_string(),
        "/sbin".to_string(),
    ];
    if let Some(home) = dirs::home_dir() {
        let h = home.display().to_string();
        v.push(format!("{}/.cargo/bin", h));
        v.push(format!("{}/.local/bin", h));
        v.push(format!("{}/bin", h));
        v.push(format!("{}/.nvm/current/bin", h));
        v.push(format!("{}/.volta/bin", h));
        v.push(format!("{}/.pyenv/shims", h));
        v.push(format!("{}/.local/share/mise/shims", h));
    }
    // fall back to the inherited PATH last (CI runners and shells often have
    // toolchains elsewhere, e.g. rustup-managed cargo)
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            if !dir.is_empty() && !v.contains(&dir.to_string()) {
                v.push(dir.to_string());
            }
        }
    }
    v
}

pub fn path_env() -> String {
    path_dirs().join(":")
}

/// Resolve a bare program name to an absolute path using PATH dirs.
pub fn resolve_program(program: &str) -> String {
    if program.contains('/') || program.starts_with('.') {
        return program.to_string();
    }
    for dir in path_dirs() {
        let candidate = format!("{}/{}", dir, program);
        if Path::new(&candidate).exists() {
            return candidate;
        }
    }
    program.to_string()
}

fn run_cli(program: &str, args: &[&str]) -> Option<String> {
    let out = StdCommand::new(resolve_program(program))
        .args(args)
        .env("PATH", path_env())
        .output()
        .ok()?;
    if !out.status.success() && out.stdout.is_empty() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim().to_string()
}

#[tauri::command]
pub async fn get_tool_versions() -> Result<ToolVersions, String> {
    let mut v = ToolVersions::default();
    if let Some(s) = run_cli("node", &["--version"]) { v.node = Some(s); }
    if let Some(s) = run_cli("npm", &["--version"]) { v.npm = Some(s); }
    if let Some(s) = run_cli("python3", &["--version"]) {
        v.python = Some(s.replace("Python ", ""));
    } else if let Some(s) = run_cli("python", &["--version"]) {
        v.python = Some(s.replace("Python ", ""));
    }
    if let Some(s) = run_cli("rustc", &["--version"]) { v.rustc = Some(s); }
    if let Some(s) = run_cli("cargo", &["--version"]) { v.cargo = Some(s); }
    if let Some(s) = run_cli("java", &["-version"]) {
        // java -version prints to stderr
        v.java = Some(first_line(&s));
    } else {
        // try stderr capture
        if let Ok(out) = StdCommand::new("java").arg("-version").output() {
            let s = String::from_utf8_lossy(&out.stderr);
            let line = first_line(&s);
            if !line.is_empty() { v.java = Some(line); }
        }
    }
    if let Some(s) = run_cli("go", &["version"]) { v.go = Some(s); }
    if let Some(s) = run_cli("git", &["--version"]) { v.git = Some(s); }
    Ok(v)
}

#[tauri::command]
pub async fn pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let folder = app.dialog().file().blocking_pick_folder();
    Ok(folder.map(|p| p.to_string()))
}

fn detect_kind(path: &Path) -> String {
    if path.join("src-tauri").join("Cargo.toml").exists() { return "tauri".into(); }
    if path.join("Cargo.toml").exists() { return "rust".into(); }
    if path.join("package.json").exists() { return "node".into(); }
    if path.join("pyproject.toml").exists() || path.join("requirements.txt").exists() || path.join("setup.py").exists() { return "python".into(); }
    if path.join("pom.xml").exists() || path.join("build.gradle").exists() || path.join("build.gradle.kts").exists() { return "java".into(); }
    if path.join("go.mod").exists() { return "go".into(); }
    if path.join("composer.json").exists() { return "php".into(); }
    if path.join("Gemfile").exists() { return "ruby".into(); }
    if path.join("Package.swift").exists() { return "swift".into(); }
    if path.join("CMakeLists.txt").exists() { return "cpp".into(); }
    if path.join("pubspec.yaml").exists() { return "flutter".into(); }
    "unknown".into()
}

fn read_json(path: &Path) -> Option<Value> {
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn parse_node(path: &Path, info: &mut ProjectInfo) {
    let pj = match read_json(&path.join("package.json")) {
        Some(v) => v,
        None => return,
    };
    info.name = pj.get("name").and_then(|v| v.as_str()).unwrap_or(&info.name).to_string();
    info.version = pj.get("version").and_then(|v| v.as_str()).map(|s| s.to_string());
    info.description = pj.get("description").and_then(|v| v.as_str()).map(|s| s.to_string());

    if let Some(scripts) = pj.get("scripts").and_then(|v| v.as_object()) {
        for (k, v) in scripts {
            info.scripts.push(Script {
                name: k.clone(),
                command: v.as_str().unwrap_or("").to_string(),
            });
        }
    }
    if let Some(deps) = pj.get("dependencies").and_then(|v| v.as_object()) {
        info.dependencies = deps.len();
    }
    if let Some(deps) = pj.get("devDependencies").and_then(|v| v.as_object()) {
        info.dev_dependencies = deps.len();
    }
}

fn parse_rust(path: &Path, info: &mut ProjectInfo) {
    let cargo_dir = if path.join("Cargo.toml").exists() { path.to_path_buf() } else if path.join("src-tauri").join("Cargo.toml").exists() { path.join("src-tauri") } else { return; };
    let cargo_path = cargo_dir.join("Cargo.toml");
    let content = match fs::read_to_string(&cargo_path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let val: toml::Value = match content.parse() {
        Ok(v) => v,
        Err(_) => return,
    };
    if let Some(pkg) = val.get("package") {
        if let Some(n) = pkg.get("name").and_then(|v| v.as_str()) { info.name = n.to_string(); }
        if let Some(v) = pkg.get("version").and_then(|v| v.as_str()) { info.version = Some(v.to_string()); }
        if let Some(d) = pkg.get("description").and_then(|v| v.as_str()) { info.description = Some(d.to_string()); }
    }
    // count deps
    if let Some(deps) = val.get("dependencies").and_then(|v| v.as_table()) {
        info.dependencies = deps.len();
    }
    if let Some(deps) = val.get("dev-dependencies").and_then(|v| v.as_table()) {
        info.dev_dependencies = deps.len();
    }
    // scripts from cargo? none standard; add common
    let common = ["build", "run", "test", "check", "clippy", "fmt"];
    for c in common {
        info.scripts.push(Script { name: c.to_string(), command: format!("cargo {}", c) });
    }
}

fn parse_python(path: &Path, info: &mut ProjectInfo) {
    // pyproject.toml
    if let Ok(content) = fs::read_to_string(path.join("pyproject.toml")) {
        if let Ok(val) = content.parse::<toml::Value>() {
            if let Some(proj) = val.get("project") {
                if let Some(n) = proj.get("name").and_then(|v| v.as_str()) { info.name = n.to_string(); }
                if let Some(v) = proj.get("version").and_then(|v| v.as_str()) { info.version = Some(v.to_string()); }
                if let Some(d) = proj.get("description").and_then(|v| v.as_str()) { info.description = Some(d.to_string()); }
                if let Some(deps) = proj.get("dependencies").and_then(|v| v.as_array()) {
                    info.dependencies = deps.len();
                }
                if let Some(scripts) = proj.get("scripts").and_then(|v| v.as_table()) {
                    for (k, v) in scripts {
                        info.scripts.push(Script { name: k.clone(), command: v.as_str().unwrap_or("").to_string() });
                    }
                }
            }
            if let Some(tool) = val.get("tool") {
                if let Some(scripts) = tool.get("scripts").and_then(|v| v.as_table()) {
                    for (k, v) in scripts {
                        info.scripts.push(Script { name: k.clone(), command: v.as_str().unwrap_or("").to_string() });
                    }
                }
            }
        }
    }
    // requirements.txt
    if path.join("requirements.txt").exists() {
        if let Ok(content) = fs::read_to_string(path.join("requirements.txt")) {
            let count = content.lines().filter(|l| !l.trim().is_empty() && !l.trim().starts_with('#')).count();
            if info.dependencies == 0 { info.dependencies = count; }
        }
    }
    // common scripts
    if info.scripts.is_empty() {
        for c in ["run", "test", "lint", "format"] {
            info.scripts.push(Script { name: c.to_string(), command: format!("python -m {}", c) });
        }
    }
}

fn parse_java(path: &Path, info: &mut ProjectInfo) {
    if let Ok(content) = fs::read_to_string(path.join("pom.xml")) {
        // name
        if let Some(n) = regex_capture(&content, r"<artifactId>([^<]+)</artifactId>") {
            if info.name == path.file_name().unwrap_or_default().to_string_lossy() {
                info.name = n;
            }
        }
        if let Some(v) = regex_capture(&content, r"<version>([^<]+)</version>") {
            info.version = Some(v);
        }
        if let Some(d) = regex_capture(&content, r"<description>([^<]+)</description>") {
            info.description = Some(d);
        }
        // deps count
        let count = content.matches("<dependency>").count();
        info.dependencies = count;
    }
    if path.join("build.gradle").exists() || path.join("build.gradle.kts").exists() {
        if info.scripts.is_empty() {
            for c in ["build", "test", "run", "clean"] {
                info.scripts.push(Script { name: c.to_string(), command: format!("./gradlew {}", c) });
            }
        }
    } else {
        if info.scripts.is_empty() {
            for c in ["compile", "test", "package", "clean"] {
                info.scripts.push(Script { name: c.to_string(), command: format!("mvn {}", c) });
            }
        }
    }
}

fn parse_go(path: &Path, info: &mut ProjectInfo) {
    if let Ok(content) = fs::read_to_string(path.join("go.mod")) {
        if let Some(m) = regex_capture(&content, r"module\s+(\S+)") {
            let name = m.split('/').last().unwrap_or(&m).to_string();
            info.name = name;
        }
        if let Some(v) = regex_capture(&content, r"go\s+(\S+)") {
            info.version = Some(v);
        }
    }
    if info.scripts.is_empty() {
        for c in ["build", "run", "test", "vet", "fmt"] {
            info.scripts.push(Script { name: c.to_string(), command: format!("go {}", c) });
        }
    }
}

fn regex_capture(text: &str, pat: &str) -> Option<String> {
    let re = regex::Regex::new(pat).ok()?;
    re.captures(text)?.get(1).map(|m| m.as_str().to_string())
}

fn parse_flutter(path: &Path, info: &mut ProjectInfo) {
    let content = match fs::read_to_string(path.join("pubspec.yaml")) {
        Ok(c) => c,
        Err(_) => return,
    };
    // simple YAML-ish parse
    let mut section = String::new();
    let mut deps = 0usize;
    let mut devdeps = 0usize;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') { continue; }
        // top-level key (no leading space)
        if !line.starts_with(' ') && trimmed.ends_with(':') {
            section = trimmed.trim_end_matches(':').to_string();
            continue;
        }
        // key: value at 2 spaces
        if line.starts_with("  ") && !line.starts_with("    ") {
            if section == "name" { /* handled below */ }
        }
        // name/version/description
        if let Some(v) = trimmed.strip_prefix("name:") {
            info.name = v.trim().trim_matches('"').to_string();
        } else if let Some(v) = trimmed.strip_prefix("version:") {
            info.version = Some(v.trim().trim_matches('"').to_string());
        } else if let Some(v) = trimmed.strip_prefix("description:") {
            info.description = Some(v.trim().trim_matches('"').to_string());
        }
        // count deps
        if section == "dependencies" && line.starts_with("  ") && !line.starts_with("    ") && trimmed.contains(':') {
            deps += 1;
        }
        if section == "dev_dependencies" && line.starts_with("  ") && !line.starts_with("    ") && trimmed.contains(':') {
            devdeps += 1;
        }
    }
    if deps > 0 { info.dependencies = deps; }
    if devdeps > 0 { info.dev_dependencies = devdeps; }
    // default scripts
    if info.scripts.is_empty() {
        for c in ["run", "build apk", "build appbundle", "test", "analyze", "pub get", "clean"] {
            info.scripts.push(Script { name: c.to_string(), command: format!("flutter {}", c) });
        }
    }
}

fn count_files(path: &Path) -> (usize, u64) {
    let mut count = 0usize;
    let mut size = 0u64;
    for entry in WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            count += 1;
            if let Ok(meta) = entry.metadata() {
                size += meta.len();
            }
            if count > 50000 { break; }
        }
    }
    (count, size)
}

fn git_info(path: &Path) -> GitInfo {
    let mut info = GitInfo::default();
    if !path.join(".git").exists() {
        return info;
    }
    info.is_repo = true;
    // branch
    if let Some(b) = run_cli("git", &["-C", path.to_str().unwrap_or(""), "rev-parse", "--abbrev-ref", "HEAD"]) {
        info.branch = Some(b);
    }
    // dirty count
    if let Some(s) = run_cli("git", &["-C", path.to_str().unwrap_or(""), "status", "--porcelain"]) {
        info.dirty_count = s.lines().count() as i64;
    }
    // ahead/behind
    if let Some(s) = run_cli("git", &["-C", path.to_str().unwrap_or(""), "rev-list", "--left-right", "--count", "HEAD...@{upstream}"]) {
        let parts: Vec<&str> = s.split_whitespace().collect();
        if parts.len() == 2 {
            info.ahead = parts[0].parse().unwrap_or(0);
            info.behind = parts[1].parse().unwrap_or(0);
        }
    }
    info
}



#[tauri::command]
pub async fn rename_project(path: String, new_name: String) -> Result<ProjectInfo, String> {
    let p = Path::new(&path);
    if !p.exists() { return Err("path does not exist".into()); }
    let kind = detect_kind(p);
    // Update the manifest on disk when supported.
    match kind.as_str() {
        "node" => {
            if let Some(mut pj) = read_json(&p.join("package.json")) {
                pj["name"] = serde_json::Value::String(new_name.clone());
                let out = serde_json::to_string_pretty(&pj).map_err(|e| e.to_string())?;
                fs::write(p.join("package.json"), out).map_err(|e| e.to_string())?;
            }
        }
        "rust" | "tauri" => {
            let cargo_dir = if p.join("Cargo.toml").exists() { p.to_path_buf() }
                else if p.join("src-tauri").join("Cargo.toml").exists() { p.join("src-tauri") }
                else { p.to_path_buf() };
            let cargo_path = cargo_dir.join("Cargo.toml");
            if cargo_path.exists() {
                let content = fs::read_to_string(&cargo_path).map_err(|e| e.to_string())?;
                let re = regex::Regex::new(r#"(?m)^(\s*name\s*=\s*")([^"]*)(")"#).unwrap();
                let new_content = re.replace(&content, |caps: &regex::Captures| {
                    format!("{}{}{}", &caps[1], new_name, &caps[3])
                }).to_string();
                fs::write(&cargo_path, new_content).map_err(|e| e.to_string())?;
            }
        }
        "flutter" => {
            let pp = p.join("pubspec.yaml");
            if pp.exists() {
                let content = fs::read_to_string(&pp).map_err(|e| e.to_string())?;
                let re = regex::Regex::new(r#"(?m)^(name:\s*).*$"#).unwrap();
                let new_content = re.replace(&content, format!("${{1}}{}", new_name)).to_string();
                fs::write(&pp, new_content).map_err(|e| e.to_string())?;
            }
        }
        "python" => {
            let pp = p.join("pyproject.toml");
            if pp.exists() {
                let content = fs::read_to_string(&pp).map_err(|e| e.to_string())?;
                let re = regex::Regex::new(r#"(?m)^(\s*name\s*=\s*")([^"]*)(")"#).unwrap();
                let new_content = re.replace(&content, |caps: &regex::Captures| {
                    format!("{}{}{}", &caps[1], new_name, &caps[3])
                }).to_string();
                fs::write(&pp, new_content).map_err(|e| e.to_string())?;
            }
        }
        _ => {}
    }
    // re-detect
    detect_project(path).await
}

#[tauri::command]
pub async fn scan_projects(path: String) -> Result<Vec<ProjectInfo>, String> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err("path does not exist".into());
    }
    let manifests = ["package.json", "Cargo.toml", "pyproject.toml", "requirements.txt", "setup.py",
                     "pom.xml", "build.gradle", "build.gradle.kts", "go.mod", "pubspec.yaml",
                     "composer.json", "Gemfile", "Package.swift", "CMakeLists.txt"];
    let has_manifest = |dir: &Path| manifests.iter().any(|m| dir.join(m).exists())
        || dir.join("src-tauri").join("Cargo.toml").exists();
    let mut out: Vec<ProjectInfo> = Vec::new();
    if has_manifest(p) {
        if let Ok(info) = detect_project(path.clone()).await {
            out.push(info);
        }
        return Ok(out);
    }
    // scan subfolders (depth 1 and 2)
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(rd) = fs::read_dir(p) {
        for e in rd.flatten() {
            let fp = e.path();
            if fp.is_dir() {
                if has_manifest(&fp) { candidates.push(fp); }
                else if let Ok(rd2) = fs::read_dir(&fp) {
                    for e2 in rd2.flatten() {
                        let fp2 = e2.path();
                        if fp2.is_dir() && has_manifest(&fp2) { candidates.push(fp2); }
                    }
                }
            }
        }
    }
    candidates.sort();
    candidates.truncate(20);
    for c in candidates {
        if let Some(s) = c.to_str() {
            if let Ok(info) = detect_project(s.to_string()).await {
                out.push(info);
            }
        }
    }
    Ok(out)
}

#[tauri::command]
pub async fn detect_project(path: String) -> Result<ProjectInfo, String> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err("path does not exist".into());
    }
    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "project".into());
    let mut info = ProjectInfo {
        path: path.clone(),
        name: name.clone(),
        kind: detect_kind(p),
        version: None,
        description: None,
        node_version: None,
        npm_version: None,
        python_version: None,
        rust_version: None,
        java_version: None,
        go_version: None,
        scripts: vec![],
        dependencies: 0,
        dev_dependencies: 0,
        files_count: 0,
        size_bytes: 0,
        git: GitInfo::default(),
    };

    match info.kind.as_str() {
        "node" => parse_node(p, &mut info),
        "rust" => parse_rust(p, &mut info),
        "tauri" => { parse_node(p, &mut info); parse_rust(p, &mut info); }
        "python" => parse_python(p, &mut info),
        "java" => parse_java(p, &mut info),
        "go" => parse_go(p, &mut info),
        "flutter" => parse_flutter(p, &mut info),
        _ => {}
    }

    // runtime versions (global)
    if let Some(s) = run_cli("node", &["--version"]) { info.node_version = Some(s); }
    if let Some(s) = run_cli("npm", &["--version"]) { info.npm_version = Some(s); }
    if let Some(s) = run_cli("python3", &["--version"]) { info.python_version = Some(s.replace("Python ", "")); }
    if let Some(s) = run_cli("rustc", &["--version"]) { info.rust_version = Some(s); }
    if let Some(s) = run_cli("java", &["-version"]).or_else(|| {
        StdCommand::new("java").arg("-version").output().ok().map(|o| String::from_utf8_lossy(&o.stderr).to_string())
    }) {
        info.java_version = Some(first_line(&s));
    }
    if let Some(s) = run_cli("go", &["version"]) { info.go_version = Some(s); }

    let (fc, sz) = count_files(p);
    info.files_count = fc;
    info.size_bytes = sz;

    info.git = git_info(p);

    Ok(info)
}

#[tauri::command]
pub async fn git_log(path: String, limit: Option<i64>) -> Result<Vec<Commit>, String> {
    let lim = limit.unwrap_or(500);
    let fmt = "%H%x1f%P%x1f%an%x1f%ad%x1f%s%x1f%D";
    let out = StdCommand::new(resolve_program("git")).env("PATH", path_env())
        .args(["-C", &path, "log", "--all", &format!("--format={}", fmt), "--date=iso", &format!("-n{}", lim)])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut commits = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() { continue; }
        let parts: Vec<&str> = line.split('\x1f').collect();
        if parts.len() < 6 { continue; }
        let hash = parts[0].to_string();
        let parents: Vec<String> = if parts[1].trim().is_empty() {
            vec![]
        } else {
            parts[1].split_whitespace().map(|s| s.to_string()).collect()
        };
        let refs: Vec<String> = parts[5].split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        commits.push(Commit {
            short_hash: hash.chars().take(7).collect(),
            hash,
            parents,
            author: parts[2].to_string(),
            date: parts[3].to_string(),
            message: parts[4].to_string(),
            refs,
        });
    }
    Ok(commits)
}

#[tauri::command]
pub async fn git_status(path: String) -> Result<String, String> {
    let out = StdCommand::new(resolve_program("git")).env("PATH", path_env())
        .args(["-C", &path, "status", "--porcelain=v1"])
        .output()
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

#[tauri::command]
pub async fn git_init(path: String) -> Result<String, String> {
    let out = StdCommand::new(resolve_program("git")).env("PATH", path_env())
        .args(["-C", &path, "init"])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    // initial commit? leave to user; create empty commit? no
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

#[tauri::command]
pub async fn run_action(
    app: AppHandle,
    state: State<'_, Arc<ProcessManager>>,
    project_path: String,
    program: String,
    args: Vec<String>,
    env: Option<Vec<(String, String)>>,
) -> Result<u32, String> {
    let env = env.unwrap_or_default();
    process_manager::spawn_command(app, state.inner().clone(), project_path, program, args, env).await
}

#[tauri::command]
pub async fn stop_process(state: State<'_, Arc<ProcessManager>>, pid: u32) -> Result<(), String> {
    process_manager::kill_process(state.inner(), pid)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn scan_flutter_subproject() {
        let infos = scan_projects("/Users/shift/Desktop/code/shift-tv".to_string()).await.unwrap();
        println!("FOUND {} projects", infos.len());
        for i in &infos { println!("  {} :: {}", i.kind, i.name); }
        assert!(infos.iter().any(|i| i.kind == "flutter"), "flutter not detected");
    }
    #[tokio::test]
    async fn scan_parent_finds_subprojects() {
        let infos = scan_projects("/Users/shift/Desktop/code".to_string()).await.unwrap();
        println!("FOUND {} projects (parent)", infos.len());
        assert!(infos.len() >= 2, "should find multiple subprojects");
    }
    #[test]
    fn resolve_program_finds_npm() {
        assert!(resolve_program("npm").starts_with('/'), "npm not resolved: {}", resolve_program("npm"));
        assert!(resolve_program("cargo").starts_with('/'));
        assert!(resolve_program("git").starts_with('/'));
    }
    #[test]
    fn path_env_includes_homebrew() {
        assert!(path_env().contains("/opt/homebrew/bin"));
    }
}
