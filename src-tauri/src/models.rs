use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Script {
    pub name: String,
    pub command: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub hash: String,
    pub short_hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub date: String,
    pub message: String,
    pub refs: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct GitInfo {
    pub is_repo: bool,
    pub branch: Option<String>,
    pub commits: Vec<Commit>,
    pub dirty_count: i64,
    pub ahead: i64,
    pub behind: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub node_version: Option<String>,
    pub npm_version: Option<String>,
    pub python_version: Option<String>,
    pub rust_version: Option<String>,
    pub java_version: Option<String>,
    pub go_version: Option<String>,
    pub scripts: Vec<Script>,
    pub dependencies: usize,
    pub dev_dependencies: usize,
    pub files_count: usize,
    pub size_bytes: u64,
    pub git: GitInfo,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToolVersions {
    pub node: Option<String>,
    pub npm: Option<String>,
    pub python: Option<String>,
    pub rustc: Option<String>,
    pub cargo: Option<String>,
    pub java: Option<String>,
    pub go: Option<String>,
    pub git: Option<String>,
}