import { useEffect, useState, useRef, useCallback, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import "./App.css";

type Script = { name: string; command: string };
type Commit = { hash: string; shortHash: string; parents: string[]; author: string; date: string; message: string; refs: string[] };
type GitInfo = { isRepo: boolean; branch: string | null; commits: Commit[]; dirtyCount: number; ahead: number; behind: number };
type ProjectInfo = {
  path: string; name: string; kind: string; version: string | null; description: string | null;
  nodeVersion: string | null; npmVersion: string | null; pythonVersion: string | null;
  rustVersion: string | null; javaVersion: string | null; goVersion: string | null;
  scripts: Script[]; dependencies: number; devDependencies: number; filesCount: number; sizeBytes: number; git: GitInfo;
};
type ToolVersions = { node: string | null; npm: string | null; python: string | null; rustc: string | null; cargo: string | null; java: string | null; go: string | null; git: string | null };
type LogEntry = { pid: number; stream: string; line: string; ts: number };
type QuickAction = { label: string; program: string; args: string[]; primary?: boolean };

const isTauri = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
const LS_KEY = "devdock.projects";

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) throw new Error("Necesita la app nativa");
  return invoke<T>(cmd, args);
}

function fmtBytes(b: number): string {
  if (b < 1024) return b + " B";
  if (b < 1024 * 1024) return (b / 1024).toFixed(1) + " KB";
  if (b < 1024 * 1024 * 1024) return (b / 1024 / 1024).toFixed(1) + " MB";
  return (b / 1024 / 1024 / 1024).toFixed(2) + " GB";
}

const KIND_ICON: Record<string, string> = {
  node: "⬢", rust: "🦀", tauri: "🦀", flutter: "🐦", python: "🐍", java: "☕", go: "🐹",
  php: "🐘", ruby: "💎", swift: "🦅", cpp: "⚙️", unknown: "📁",
};
const KIND_LABEL: Record<string, string> = {
  node: "Node / JS", rust: "Rust", tauri: "Tauri (Rust+JS)", flutter: "Flutter (Dart)", python: "Python",
  java: "Java", go: "Go", php: "PHP", ruby: "Ruby", swift: "Swift", cpp: "C++", unknown: "Proyecto",
};
const kindIcon = (k: string) => KIND_ICON[k] || "📁";
const kindLabel = (k: string) => KIND_LABEL[k] || k;

function quickActionsFor(p: ProjectInfo): QuickAction[] {
  switch (p.kind) {
    case "node":
      return [
        { label: "▶ Dev", program: "npm", args: ["run", "dev"], primary: true },
        { label: "⚙ Build", program: "npm", args: ["run", "build"] },
        { label: "▶ Prod", program: "npm", args: ["run", "start"] },
        { label: "🧪 Test", program: "npm", args: ["run", "test"] },
      ];
    case "rust":
      return [
        { label: "▶ Run", program: "cargo", args: ["run"], primary: true },
        { label: "⚙ Build rel", program: "cargo", args: ["build", "--release"] },
        { label: "🧪 Test", program: "cargo", args: ["test"] },
        { label: "✓ Check", program: "cargo", args: ["check"] },
      ];
    case "tauri":
      return [
        { label: "▶ Tauri dev", program: "npm", args: ["run", "tauri", "dev"], primary: true },
        { label: "▶ Vite dev", program: "npm", args: ["run", "dev"] },
        { label: "⚙ Build", program: "npm", args: ["run", "tauri", "build"] },
        { label: "🧪 cargo test", program: "cargo", args: ["test", "--manifest-path", "src-tauri/Cargo.toml"] },
      ];
    case "python":
      return [
        { label: "▶ py main", program: "python3", args: ["main.py"], primary: true },
        { label: "🧪 pytest", program: "pytest", args: [] },
        { label: "📦 pip reqs", program: "pip3", args: ["install", "-r", "requirements.txt"] },
      ];
    case "java":
      return [
        { label: "⚙ Build", program: "./gradlew", args: ["build"], primary: true },
        { label: "🧪 Test", program: "./gradlew", args: ["test"] },
        { label: "▶ Run", program: "./gradlew", args: ["bootRun"] },
      ];
    case "go":
      return [
        { label: "▶ Run", program: "go", args: ["run", "."], primary: true },
        { label: "⚙ Build", program: "go", args: ["build", "./..."] },
        { label: "🧪 Test", program: "go", args: ["test", "./..."] },
      ];
    case "flutter":
      return [
        { label: "▶ Run", program: "flutter", args: ["run"], primary: true },
        { label: "⚙ Build apk", program: "flutter", args: ["build", "apk"] },
        { label: "📦 pub get", program: "flutter", args: ["pub", "get"] },
        { label: "🧪 Test", program: "flutter", args: ["test"] },
        { label: "🔍 Analyze", program: "flutter", args: ["analyze"] },
      ];
    default:
      return [];
  }
}

export default function App() {
  const [projects, setProjects] = useState<ProjectInfo[]>([]);
  const [detailPath, setDetailPath] = useState<string | null>(null);
  const [detailTab, setDetailTab] = useState<"info" | "scripts" | "git" | "logs">("info");
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [running, setRunning] = useState<Record<number, { path: string; program: string }>>({});
  const [toolVersions, setToolVersions] = useState<ToolVersions | null>(null);
  const [gitCommits, setGitCommits] = useState<Commit[]>([]);
  const [loading, setLoading] = useState<string[]>([]);
  const [dragOver, setDragOver] = useState(false);
  const [errors, setErrors] = useState<string[]>([]);
  const [customCmd, setCustomCmd] = useState("");
  const [pendingUpdate, setPendingUpdate] = useState<{ version: string; downloadAndInstall: () => Promise<void> } | null>(null);
  const [pidHistory, setPidHistory] = useState<Record<number, string>>({});
  const logEndRef = useRef<HTMLDivElement>(null);

  const detailProject = projects.find((p) => p.path === detailPath) || null;

  const notify = useCallback((msg: string) => {
    setErrors((prev) => [...prev.slice(-4), msg]);
  }, []);

  const savedPaths = useCallback((): string[] => {
    try { return JSON.parse(localStorage.getItem(LS_KEY) || "[]"); } catch { return []; }
  }, []);

  const savePaths = useCallback((paths: string[]) => {
    try { localStorage.setItem(LS_KEY, JSON.stringify(paths)); } catch {}
  }, []);

  const addProjects = useCallback(async (path: string, select = true) => {
    setLoading((prev) => [...new Set([...prev, path])]);
    try {
      const infos = await call<ProjectInfo[]>("scan_projects", { path });
      if (!infos || infos.length === 0) {
        notify(`No se detectaron proyectos en ${path}`);
        return;
      }
      setProjects((prev) => {
        const filtered = prev.filter((p) => !infos.some((i) => i.path === p.path));
        return [...infos, ...filtered];
      });
      if (select && infos.length === 1) setDetailPath(infos[0].path);
      const newPaths = infos.map((i) => i.path);
      const paths = savedPaths().filter((x) => !newPaths.includes(x));
      savePaths([...newPaths, ...paths]);
    } catch (e: any) {
      notify(`No se pudo leer ${path}: ${String(e)}`);
    } finally {
      setLoading((prev) => prev.filter((x) => x !== path));
    }
  }, [notify, savedPaths, savePaths]);

  const refresh = useCallback(async (path: string) => {
    setLoading((prev) => [...new Set([...prev, path])]);
    try {
      const info = await call<ProjectInfo>("detect_project", { path });
      setProjects((prev) => prev.map((p) => (p.path === path ? info : p)));
    } catch (e: any) { notify(`Error refrescando: ${String(e)}`); }
    finally { setLoading((prev) => prev.filter((x) => x !== path)); }
  }, [notify]);

  // persistencia: cargar proyectos al abrir
  useEffect(() => {
    const paths = savedPaths();
    paths.forEach((p) => addProjects(p, false));
    call<ToolVersions>("get_tool_versions").then(setToolVersions).catch(() => {});
    // auto-update check
    if (isTauri()) {
      check().then((u) => {
        if (u) setPendingUpdate({ version: u.version, downloadAndInstall: () => u.downloadAndInstall() });
      }).catch((e) => notify(`Update check: ${String(e)}`));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // drag & drop nativo
  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: (() => void) | undefined;
    getCurrentWebview().onDragDropEvent((event: any) => {
      const payload = (event && event.payload) ? event.payload : event;
      if (payload.type === "over") setDragOver(true);
      else if (payload.type === "leave" || payload.type === "cancelled") setDragOver(false);
      else if (payload.type === "drop") {
        setDragOver(false);
        (payload.paths || []).forEach((p: string) => addProjects(p, false));
      }
    }).then((fn: any) => { unlisten = fn; });
    return () => { if (unlisten) unlisten(); };
  }, [addProjects]);

  // eventos de procesos
  useEffect(() => {
    if (!isTauri()) return;
    let un1: (() => void) | undefined;
    let un2: (() => void) | undefined;
    listen("process:log", (e: any) => {
      const p = e.payload;
      setLogs((prev) => [...prev.slice(-800), { pid: p.pid, stream: p.stream, line: p.line, ts: Date.now() }]);
    }).then((fn: any) => { un1 = fn; });
    listen("process:exit", (e: any) => {
      const p = e.payload;
      setLogs((prev) => [...prev, { pid: p.pid, stream: "exit", line: `[proceso ${p.pid} terminó con código ${p.code}]`, ts: Date.now() }]);
      setRunning((prev) => { const next = { ...prev }; delete next[p.pid]; return next; });
    }).then((fn: any) => { un2 = fn; });
    return () => { if (un1) un1(); if (un2) un2(); };
  }, []);

  useEffect(() => { logEndRef.current?.scrollIntoView({ behavior: "smooth" }); }, [logs, detailTab]);

  const runAction = useCallback(async (path: string, program: string, args: string[]) => {
    try {
      const pid = await call<number>("run_action", { projectPath: path, program, args, env: null });
      setRunning((prev) => ({ ...prev, [pid]: { path, program: [program, ...args].join(" ") } }));
      setPidHistory((prev) => ({ ...prev, [pid]: path }));
      return pid;
    } catch (e: any) {
      notify(`⚠ ${program}: ${String(e)}`);
      return null;
    }
  }, [notify]);

  const runNpmScript = useCallback(async (path: string, name: string) => {
    return runAction(path, "npm", ["run", name]);
  }, [runAction]);

  const stopProc = async (pid: number) => {
    try {
      await call("stop_process", { pid });
      setRunning((prev) => { const next = { ...prev }; delete next[pid]; return next; });
      setLogs((prev) => [...prev, { pid, stream: "exit", line: `[proceso ${pid} detenido]`, ts: Date.now() }]);
    } catch (e: any) { notify(String(e)); }
  };

  const removeProject = useCallback((path: string) => {
    setProjects((prev) => prev.filter((p) => p.path !== path));
    savePaths(savedPaths().filter((x) => x !== path));
    if (detailPath === path) setDetailPath(null);
  }, [detailPath, savedPaths, savePaths]);

  const handlePickFolder = async () => {
    try {
      const folder = await open({ directory: true, multiple: false });
      if (folder) await addProjects(folder as string, false);
    } catch (e: any) { notify(String(e)); }
  };

  // detalle: cargar git log al abrir
  useEffect(() => {
    if (!detailPath) { setGitCommits([]); return; }
    const proj = projects.find((p) => p.path === detailPath);
    if (!proj || !proj.git.isRepo) { setGitCommits([]); return; }
    call<Commit[]>("git_log", { path: detailPath, limit: 300 }).then(setGitCommits).catch(() => setGitCommits([]));
  }, [detailPath]); // eslint-disable-line

  const handleGitInit = async () => {
    if (!detailPath) return;
    try {
      await call("git_init", { path: detailPath });
      await refresh(detailPath);
      const commits = await call<Commit[]>("git_log", { path: detailPath, limit: 300 });
      setGitCommits(commits);
    } catch (e: any) { notify(`git init: ${String(e)}`); }
  };

  // ⌘1-4: switch drawer tabs
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!e.metaKey || !detailProject) return;
      const map: Record<string, typeof detailTab> = { "1": "info", "2": "scripts", "3": "git", "4": "logs" };
      if (map[e.key]) { e.preventDefault(); setDetailTab(map[e.key]); }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [detailProject]);

  const gitLayout = useMemo(() => {
    const indexByHash = new Map<string, number>();
    gitCommits.forEach((c, i) => indexByHash.set(c.hash, i));
    const lanes: (string | null)[] = [];
    const laneOf: number[] = [];
    gitCommits.forEach((c) => {
      let lane = lanes.indexOf(c.hash);
      if (lane === -1) {
        lane = lanes.indexOf(null);
        if (lane === -1) { lane = lanes.length; lanes.push(null); }
      }
      laneOf.push(lane);
      lanes[lane] = null;
      c.parents.forEach((p, idx) => {
        let pl = lanes.indexOf(p);
        if (pl === -1) {
          pl = lanes.indexOf(null);
          if (pl === -1) { pl = lanes.length; lanes.push(null); }
          if (idx === 0) lanes[pl] = p; else lanes[pl] = p;
        }
      });
    });
    const maxLane = Math.max(0, ...laneOf);
    return { laneOf, maxLane, indexByHash };
  }, [gitCommits]);

  const pidsForPath = (path: string) =>
    Object.entries(running).filter(([, r]) => r.path === path).map(([pid]) => Number(pid));

  const stopProject = async (path: string) => {
    const pids = pidsForPath(path);
    if (pids.length === 0) { notify("No hay procesos activos registrados para este proyecto"); return; }
    for (const pid of pids) {
      try {
        await call("stop_process", { pid });
        // optimistic: backend already SIGKILLed the whole group
        setRunning((prev) => { const next = { ...prev }; delete next[pid]; return next; });
        setLogs((prev) => [...prev, { pid, stream: "exit", line: `[proceso ${pid} detenido]`, ts: Date.now() }]);
      } catch (e: any) { notify(String(e)); }
    }
  };

  const detailLogs = detailPath
    ? logs.filter((l) => pidHistory[l.pid] === detailPath)
    : logs;

  return (
    <div className="app">
      <div className="topbar" data-tauri-drag-region>
        <h1 className="brand">DevDock</h1>
        {pendingUpdate && (
          <button className="btn primary sm" onClick={async () => {
            try {
              notify("Descargando actualización…");
              await pendingUpdate.downloadAndInstall();
              await relaunch();
            } catch (e: any) { notify(`Update: ${String(e)}`); }
          }}>↑ v{pendingUpdate.version} disponible — actualizar</button>
        )}
        <div className="spacer" />
        {toolVersions && (
          <div className="tools">
            {toolVersions.node && <span className="chip">node {toolVersions.node}</span>}
            {toolVersions.npm && <span className="chip">npm {toolVersions.npm}</span>}
            {toolVersions.python && <span className="chip">py {toolVersions.python}</span>}
            {toolVersions.cargo && <span className="chip">cargo {toolVersions.cargo?.split(" ")[1]}</span>}
            {toolVersions.go && <span className="chip">go {toolVersions.go?.split(" ")[2]}</span>}
            {toolVersions.git && <span className="chip">git {toolVersions.git?.split(" ")[2]}</span>}
          </div>
        )}
        <button className="btn primary" onClick={handlePickFolder}>+ Añadir proyecto</button>
      </div>

      {errors.length > 0 && (
        <div className="toasts">
          {errors.map((e, i) => (
            <div className="toast" key={i} onClick={() => setErrors((prev) => prev.filter((_, j) => j !== i))}>{e}</div>
          ))}
        </div>
      )}

      <div className={"dash" + (dragOver ? " dragging" : "")}>
        {projects.length === 0 ? (
          <div className="empty-dash">
            {dragOver ? (
              <>
                <div className="big">⬇</div>
                <h2>Suelta las carpetas aquí</h2>
              </>
            ) : loading.length > 0 ? (
              <><span className="spinner" />Analizando proyectos…</>
            ) : (
              <>
                <div className="big">📦</div>
                <h2>Arrastra proyectos aquí</h2>
                <p>Carpetas de Node, Rust, Tauri, Python, Java, Go…</p>
                <button className="btn primary" onClick={handlePickFolder}>O buscar carpeta</button>
              </>
            )}
          </div>
        ) : (
          <div className="grid">
            {projects.map((p) => {
              const isRunning = pidsForPath(p.path).length > 0;
              const isLoading = loading.includes(p.path);
              const qa = quickActionsFor(p);
              return (
                <div className="card proj" key={p.path} onClick={() => { setDetailPath(p.path); setDetailTab("info"); }}>
                  <div className="proj-head">
                    <span className="proj-icon">{kindIcon(p.kind)}</span>
                    <div className="proj-title">
                      <div className="proj-name">{p.name}</div>
                      <div className="proj-kind">{kindLabel(p.kind)} {p.version ? `· v${p.version}` : ""}</div>
                    </div>
                    <div className="status-wrap">
                      {isLoading && <span className="spinner" />}
                      {isRunning && <span className="badge running">● RUNNING</span>}
                    </div>
                  </div>

                  <div className="metrics-row">
                    <div className="m"><span className="ml">Deps</span><span className="mv">{p.dependencies}</span></div>
                    <div className="m"><span className="ml">Dev</span><span className="mv">{p.devDependencies}</span></div>
                    <div className="m"><span className="ml">Archivos</span><span className="mv">{p.filesCount.toLocaleString()}</span></div>
                    <div className="m"><span className="ml">Tamaño</span><span className="mv">{fmtBytes(p.sizeBytes)}</span></div>
                  </div>

                  <div className="runtime-row">
                    {p.kind === "node" && p.nodeVersion && <span className="chip">node {p.nodeVersion}</span>}
                    {p.kind === "node" && p.npmVersion && <span className="chip">npm {p.npmVersion}</span>}
                    {p.kind === "rust" && p.rustVersion && <span className="chip">{p.rustVersion.split(" ")[1]}</span>}
                    {p.kind === "python" && p.pythonVersion && <span className="chip">py {p.pythonVersion}</span>}
                    {p.kind === "java" && p.javaVersion && <span className="chip">java</span>}
                    {p.kind === "go" && p.goVersion && <span className="chip">go</span>}
                  </div>

                  <div className="git-row">
                    {p.git.isRepo ? (
                      <>
                        <span className="chip git">⑂ {p.git.branch || "—"}</span>
                        {p.git.dirtyCount > 0 && <span className="chip dirty">±{p.git.dirtyCount}</span>}
                        {p.git.ahead > 0 && <span className="chip">↑{p.git.ahead}</span>}
                        {p.git.behind > 0 && <span className="chip">↓{p.git.behind}</span>}
                      </>
                    ) : (
                      <span className="chip nogit">sin git</span>
                    )}
                  </div>

                  <div className="card-actions" onClick={(e) => e.stopPropagation()}>
                    {isRunning ? (
                      <button className="btn sm danger" onClick={() => stopProject(p.path)}>■ Stop</button>
                    ) : (
                      qa[0] && (
                        <button className="btn sm primary"
                          onClick={() => runAction(p.path, qa[0].program, qa[0].args)}>{qa[0].label}</button>
                      )
                    )}
                    {qa.slice(1, 3).map((a) => (
                      <button key={a.label} className={"btn sm" + (a.primary ? " primary" : "")}
                        onClick={() => runAction(p.path, a.program, a.args)}>{a.label}</button>
                    ))}
                    <button className="btn sm" onClick={() => { setDetailPath(p.path); setDetailTab("scripts"); }}>⋯</button>
                    <button className="btn sm icon" title="Refrescar" onClick={() => refresh(p.path)}>↻</button>
                    <button className="btn sm icon danger" title="Quitar" onClick={() => removeProject(p.path)}>✕</button>
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>

      {detailProject && (
        <div className="drawer-overlay" onClick={() => setDetailPath(null)}>
          <aside className="drawer" onClick={(e) => e.stopPropagation()}>
            <div className="drawer-head">
              <span className="proj-icon">{kindIcon(detailProject.kind)}</span>
              <div>
                <div className="proj-name">{detailProject.name}</div>
                <div className="path-sm">{detailProject.path}</div>
              </div>
              <div className="spacer" />
              <button className="btn sm icon" onClick={() => setDetailPath(null)}>✕</button>
            </div>
            <div className="tabs">
              {(["info", "scripts", "git", "logs"] as const).map((t) => (
                <button key={t} className={"tab" + (detailTab === t ? " active" : "")} onClick={() => setDetailTab(t)}>
                  {{ info: "Resumen", scripts: "Scripts", git: "Git", logs: "Logs" }[t]}
                </button>
              ))}
            </div>
            <div className="drawer-body">
              {detailTab === "info" && (
                <>
                  <div className="card">
                    <h2>Estado Git</h2>
                    {detailProject.git.isRepo ? (
                      <div className="metrics-row">
                        <div className="m"><span className="ml">Rama</span><span className="mv">{detailProject.git.branch || "—"}</span></div>
                        <div className="m"><span className="ml">Cambios</span><span className="mv">{detailProject.git.dirtyCount}</span></div>
                        <div className="m"><span className="ml">Ahead</span><span className="mv">{detailProject.git.ahead}</span></div>
                        <div className="m"><span className="ml">Behind</span><span className="mv">{detailProject.git.behind}</span></div>
                      </div>
                    ) : (
                      <div className="row-center">
                        <span className="muted">Este proyecto no tiene repositorio Git.</span>
                        <button className="btn primary" onClick={handleGitInit}>Inicializar Git</button>
                      </div>
                    )}
                  </div>
                  <div className="card">
                    <h2>Acciones</h2>
                    <div className="actions">
                      {quickActionsFor(detailProject).map((a) => (
                        <button key={a.label} className={"btn" + (a.primary ? " primary" : "")}
                          onClick={() => runAction(detailProject.path, a.program, a.args)}>{a.label}</button>
                      ))}
                    </div>
                  </div>
                  <div className="card">
                    <h2>Procesos</h2>
                    {pidsForPath(detailProject.path).length === 0 ? (
                      <span className="muted">Nada corriendo</span>
                    ) : (
                      Object.entries(running).filter(([, r]) => r.path === detailProject.path).map(([pid, r]) => (
                        <div className="script-row" key={pid}>
                          <div><span className="mono">PID {pid}</span> <span className="scmd">{r.program}</span></div>
                          <button className="btn sm danger" onClick={() => stopProc(Number(pid))}>■ Detener</button>
                        </div>
                      ))
                    )}
                  </div>
                </>
              )}

              {detailTab === "scripts" && (
                <div className="card">
                  <h2>Scripts</h2>
                  {detailProject.scripts.length === 0 && <span className="muted">Sin scripts declarados</span>}
                  {detailProject.scripts.map((s) => (
                    <div className="script-row" key={s.name}>
                      <div><div className="mono">{s.name}</div><div className="scmd">{s.command}</div></div>
                      <button className="btn sm primary" onClick={() => {
                        if (detailProject.kind === "node") runNpmScript(detailProject.path, s.name);
                        else {
                          const parts = s.command.trim().split(/\s+/);
                          runAction(detailProject.path, parts[0], parts.slice(1));
                        }
                      }}>▶</button>
                    </div>
                  ))}
                  <h2 style={{ marginTop: 16 }}>Comando personalizado</h2>
                  <div className="toolbar">
                    <input type="text" value={customCmd} onChange={(e) => setCustomCmd(e.target.value)}
                      placeholder="ej: npm run dev -- --port 3000"
                      onKeyDown={async (e) => {
                        if (e.key === "Enter" && customCmd.trim()) {
                          const parts = customCmd.trim().split(/\s+/);
                          await runAction(detailProject.path, parts[0], parts.slice(1));
                          setCustomCmd("");
                        }
                      }} />
                    <button className="btn primary" onClick={async () => {
                      if (customCmd.trim()) {
                        const parts = customCmd.trim().split(/\s+/);
                        await runAction(detailProject.path, parts[0], parts.slice(1));
                        setCustomCmd("");
                      }
                    }}>▶</button>
                  </div>
                </div>
              )}

              {detailTab === "git" && (
                <div className="card">
                  {!detailProject.git.isRepo ? (
                    <div className="row-center">
                      <span className="muted">Sin repositorio Git.</span>
                      <button className="btn primary" onClick={handleGitInit}>Inicializar Git</button>
                    </div>
                  ) : (
                    <>
                      <h2>Historial ({gitCommits.length})</h2>
                      {gitCommits.length === 0 ? <span className="muted">Sin commits</span> : (
                        <div className="graph-wrap">
                          <svg width={gitLayout.maxLane * 20 + 24} height={gitCommits.length * 26 + 10}>
                            {gitCommits.map((c, i) => {
                              const x = gitLayout.laneOf[i] * 20 + 10;
                              const y = i * 26 + 14;
                              const color = `hsl(${(gitLayout.laneOf[i] * 60 + 220) % 360}, 70%, 65%)`;
                              return (
                                <g key={c.hash}>
                                  {c.parents.map((par) => {
                                    const j = gitLayout.indexByHash.get(par);
                                    if (j === undefined) return null;
                                    return <line key={par} x1={x} y1={y} x2={gitLayout.laneOf[j] * 20 + 10} y2={j * 26 + 14} stroke={color} strokeWidth={2} />;
                                  })}
                                  <circle cx={x} cy={y} r={4.5} fill={color} stroke="#0b0d14" strokeWidth={1.5} />
                                </g>
                              );
                            })}
                          </svg>
                          <div className="commit-list">
                            {gitCommits.map((c) => (
                              <div className="commit-row" key={c.hash}>
                                <div style={{ minWidth: 54 }}>
                                  <div className="mono accent">{c.shortHash}</div>
                                  <div className="commit-meta">{c.author.split(" ")[0]} · {c.date.slice(0, 10)}</div>
                                </div>
                                <div className="commit-msg">
                                  {c.refs.map((r) => <span key={r} className={"ref-badge" + (r.includes("HEAD") ? " head" : "")}>{r}</span>)}
                                  {c.message}
                                </div>
                              </div>
                            ))}
                          </div>
                        </div>
                      )}
                    </>
                  )}
                </div>
              )}

              {detailTab === "logs" && (
                <div className="card">
                  <h2>Logs</h2>
                  <div className="logbox">
                    {detailLogs.map((l, i) => (
                      <div key={i} className={l.stream === "stderr" ? "stderr" : l.stream === "exit" ? "exitline" : "stdout"}>{l.line}</div>
                    ))}
                    <div ref={logEndRef} />
                  </div>
                  <div className="actions" style={{ marginTop: 10 }}>
                    <button className="btn sm" onClick={() => {
                      const mine = Object.entries(pidHistory).filter(([, path]) => path === detailProject.path).map(([pid]) => Number(pid));
                      setLogs((prev) => prev.filter((l) => !mine.includes(l.pid)));
                    }}>Limpiar</button>
                  </div>
                </div>
              )}
            </div>
          </aside>
        </div>
      )}
    </div>
  );
}