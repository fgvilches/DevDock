use std::collections::HashMap;
use std::process::{Command as StdCommand, Stdio};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::commands::{path_env, resolve_program};

pub struct ProcessManager {
    pub children: Mutex<HashMap<u32, Child>>,
}

impl ProcessManager {
    pub fn new() -> Self {
        Self { children: Mutex::new(HashMap::new()) }
    }
}

pub async fn spawn_command(
    app: AppHandle,
    manager: Arc<ProcessManager>,
    project_path: String,
    program: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
) -> Result<u32, String> {
    let resolved = resolve_program(&program);
    let mut cmd = Command::new(&resolved);
    cmd.args(&args)
        .current_dir(&project_path)
        .env("PATH", path_env())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // own process group so we can kill the whole tree (npm -> vite/node) later
    #[cfg(unix)]
    cmd.process_group(0);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("no se pudo iniciar `{program}` desde {project_path}: {e}"))?;
    let pid = child.id().ok_or_else(|| "no pid".to_string())?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    {
        let mut map = manager.children.lock().unwrap();
        map.insert(pid, child);
    }

    let app_clone = app.clone();
    let pid_u = pid;
    if let Some(out) = stdout {
        tokio::spawn(async move {
            let mut reader = BufReader::new(out).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let _ = app_clone.emit("process:log", serde_json::json!({
                    "pid": pid_u,
                    "stream": "stdout",
                    "line": line,
                }));
            }
        });
    }
    let app_clone2 = app.clone();
    if let Some(err) = stderr {
        tokio::spawn(async move {
            let mut reader = BufReader::new(err).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let _ = app_clone2.emit("process:log", serde_json::json!({
                    "pid": pid_u,
                    "stream": "stderr",
                    "line": line,
                }));
            }
        });
    }

    // wait task to emit exit
    let app_clone3 = app.clone();
    let manager_clone = manager.clone();
    tokio::spawn(async move {
        // poll child status
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let mut map = manager_clone.children.lock().unwrap();
            if let Some(child) = map.get_mut(&pid_u) {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        let code = status.code().unwrap_or(-1);
                        let _ = app_clone3.emit("process:exit", serde_json::json!({
                            "pid": pid_u,
                            "code": code,
                        }));
                        map.remove(&pid_u);
                        break;
                    }
                    Ok(None) => {}
                    Err(_) => {
                        map.remove(&pid_u);
                        break;
                    }
                }
            } else {
                break;
            }
        }
    });

    Ok(pid)
}

pub fn kill_process(manager: &ProcessManager, pid: u32) -> Result<(), String> {
    let has_child = manager.children.lock().unwrap().contains_key(&pid);
    if !has_child {
        return Err(format!("PID {} no pertenece a DevDock", pid));
    }
    // kill the whole process group (children like vite/node die too).
    // pgid == pid because we spawn with process_group(0).
    // NOTE: /usr/bin/kill misparses negative pids on macOS ("kill -9 -123" reads -1,-2,-3 as flags),
    // so we call kill(2) directly via libc.
    let killed = unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    if killed == 0 {
        return Ok(());
    }
    // fallback: wrapper only
    {
        let mut map = manager.children.lock().unwrap();
        if let Some(child) = map.get_mut(&pid) {
            let _ = child.start_kill();
            Ok(())
        } else {
            Err("proceso no encontrado".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command as StdCommand, Stdio};

    #[tokio::test]
    async fn kill_kills_npm_tree() {
        // reproduce the real case: npm run dev spawns vite as a child tree
        let mut cmd = Command::new("npm");
        cmd.args(["run", "dev", "--", "--port", "14999", "--strictPort"])
            .current_dir("/Users/shift/Desktop/code/DevDock")
            .process_group(0)
            .env("PATH", crate::commands::path_env())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("npm spawn failed");
        let pid = child.id().unwrap();
        let mgr = ProcessManager::new();
        mgr.children.lock().unwrap().insert(pid, child);

        // wait until vite actually listens (max 15s)
        let mut listening = false;
        for _ in 0..30 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let out = StdCommand::new("lsof").args(["-nP", "-iTCP:14999", "-sTCP:LISTEN"]).output().unwrap();
            let txt = String::from_utf8_lossy(&out.stdout).to_string();
            let rows: Vec<&str> = txt.lines().filter(|l| l.contains("LISTEN")).collect();
            if !rows.is_empty() {
                println!("LISTENING rows={} : {}", rows.len(), rows[0]);
                listening = true;
                break;
            }
        }
        assert!(listening, "vite never started listening");

        // group members before kill (wrapper + npm-sh + vite/node)
        let before = StdCommand::new("pgrep").args(["-g", &pid.to_string()]).output().unwrap();
        println!("GROUP MEMBERS BEFORE KILL: {:?}", String::from_utf8_lossy(&before.stdout).trim());

        kill_process(&mgr, pid).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;

        // group empty after kill
        let after = StdCommand::new("pgrep").args(["-g", &pid.to_string()]).output().unwrap();
        let txt = String::from_utf8_lossy(&after.stdout).trim().to_string();
        assert!(txt.is_empty(), "npm tree survived kill: pids=[{}]", txt);

        // port released
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        let out = StdCommand::new("lsof").args(["-nP", "-iTCP:14999", "-sTCP:LISTEN"]).output().unwrap();
        let txt2 = String::from_utf8_lossy(&out.stdout).to_string();
        let rows2: Vec<&str> = txt2.lines().filter(|l| l.contains("LISTEN")).collect();
        assert!(rows2.is_empty(), "port 14999 still listening after kill");
    }
}
