//! SQLite persistence (ADR 0010) on its own thread, driven by messages.
//!
//! Stores projects, groups, agents, rules, the UI layout and the decision history.
//! Pending permission requests and API keys are never written here.

use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
};

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::model::{Agent, AgentState, Group, PermissionRule, Project, Provider};

pub struct LoadedProject {
    pub project: Project,
    pub groups: Vec<Group>,
    pub agents: Vec<Agent>,
    pub layout: Option<String>,
}

pub struct HistoryEntry {
    pub agent_id: Uuid,
    pub tool: String,
    /// Command or path only; the raw input may hold secrets.
    pub summary: String,
    pub decision: String,
    pub source: String,
}

enum Op {
    SaveProject(Project),
    SaveGroup(Group),
    SaveAgent(Box<Agent>),
    DeleteAgent(Uuid),
    SaveLayout {
        project: Uuid,
        layout: String,
    },
    History(HistoryEntry),
    SetSetting {
        key: String,
        value: String,
    },
    Setting {
        key: String,
        reply: oneshot::Sender<Option<String>>,
    },
    Load {
        root: PathBuf,
        reply: oneshot::Sender<Option<LoadedProject>>,
    },
    /// The project was opened now (for the recent list).
    Opened(Uuid),
    Recent {
        limit: usize,
        reply: oneshot::Sender<Vec<PathBuf>>,
    },
}

#[derive(Clone)]
pub struct Store {
    tx: mpsc::Sender<Op>,
}

impl Store {
    /// `$ORCH_DB`, or `~/.local/share/orchestrator/orchestrator.db`.
    pub fn default_path() -> PathBuf {
        std::env::var_os("ORCH_DB").map_or_else(|| crate::voice::data_dir().join("orchestrator.db"), PathBuf::from)
    }

    pub fn open(path: &Path) -> Result<Store, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let connection = Connection::open(path).map_err(|e| e.to_string())?;
        migrate(&connection).map_err(|e| e.to_string())?;
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("sqlite".into())
            .spawn(move || {
                while let Ok(op) = rx.recv() {
                    if let Err(error) = apply(&connection, op) {
                        tracing::error!(%error, "sqlite write failed");
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Store { tx })
    }

    fn send(&self, op: Op) {
        let _ = self.tx.send(op);
    }

    pub fn save_project(&self, project: &Project) {
        self.send(Op::SaveProject(project.clone()));
    }

    pub fn save_group(&self, group: &Group) {
        self.send(Op::SaveGroup(group.clone()));
    }

    pub fn save_agent(&self, agent: &Agent) {
        self.send(Op::SaveAgent(Box::new(agent.clone())));
    }

    pub fn delete_agent(&self, id: Uuid) {
        self.send(Op::DeleteAgent(id));
    }

    pub fn save_layout(&self, project: Uuid, layout: String) {
        self.send(Op::SaveLayout { project, layout });
    }

    pub fn record(&self, entry: HistoryEntry) {
        self.send(Op::History(entry));
    }

    pub fn set_setting(&self, key: &str, value: &str) {
        self.send(Op::SetSetting { key: key.into(), value: value.into() });
    }

    pub async fn setting(&self, key: &str) -> Option<String> {
        let (reply, value) = oneshot::channel();
        self.send(Op::Setting { key: key.into(), reply });
        value.await.ok().flatten()
    }

    pub fn opened(&self, project: Uuid) {
        self.send(Op::Opened(project));
    }

    /// Roots of the projects opened most recently, newest first.
    pub async fn recent(&self, limit: usize) -> Vec<PathBuf> {
        let (reply, roots) = oneshot::channel();
        self.send(Op::Recent { limit, reply });
        roots.await.unwrap_or_default()
    }

    pub async fn load(&self, root: &Path) -> Option<LoadedProject> {
        let (reply, project) = oneshot::channel();
        self.send(Op::Load { root: root.to_path_buf(), reply });
        project.await.ok().flatten()
    }
}

fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    migrate_tables(connection)?;
    // Columns added after the first release; the error just means it exists.
    let _ = connection.execute("ALTER TABLE groups ADD COLUMN max_active INTEGER", []);
    let _ = connection.execute("ALTER TABLE agents ADD COLUMN persona TEXT NOT NULL DEFAULT ''", []);
    let _ = connection.execute("ALTER TABLE projects ADD COLUMN opened_at INTEGER NOT NULL DEFAULT 0", []);
    let _ = connection.execute("ALTER TABLE agents ADD COLUMN model TEXT", []);
    let _ = connection.execute("ALTER TABLE agents ADD COLUMN color TEXT", []);
    Ok(())
}

fn migrate_tables(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE IF NOT EXISTS projects (
             id TEXT PRIMARY KEY, name TEXT NOT NULL, root TEXT NOT NULL UNIQUE, layout TEXT);
         CREATE TABLE IF NOT EXISTS groups (
             id TEXT PRIMARY KEY, project_id TEXT NOT NULL, name TEXT NOT NULL,
             goal TEXT NOT NULL, rules TEXT NOT NULL DEFAULT '[]', position INTEGER);
         CREATE TABLE IF NOT EXISTS agents (
             id TEXT PRIMARY KEY, group_id TEXT NOT NULL, name TEXT NOT NULL, task TEXT NOT NULL,
             provider TEXT NOT NULL, worktree TEXT NOT NULL UNIQUE, rules TEXT NOT NULL DEFAULT '[]',
             position INTEGER);
         CREATE TABLE IF NOT EXISTS history (
             id INTEGER PRIMARY KEY AUTOINCREMENT, agent_id TEXT NOT NULL, tool TEXT NOT NULL,
             summary TEXT NOT NULL, decision TEXT NOT NULL, source TEXT NOT NULL, decided_at TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )
}

fn json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".into())
}

fn apply(db: &Connection, op: Op) -> rusqlite::Result<()> {
    match op {
        Op::SaveProject(p) => {
            db.execute(
                "INSERT INTO projects (id, name, root) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET name = excluded.name, root = excluded.root",
                params![p.id.to_string(), p.name, p.root.to_string_lossy()],
            )?;
        }
        Op::SaveGroup(g) => {
            db.execute(
                "INSERT INTO groups (id, project_id, name, goal, rules, position, max_active)
                 VALUES (?1, ?2, ?3, ?4, ?5, (SELECT COUNT(*) FROM groups), ?6)
                 ON CONFLICT(id) DO UPDATE SET name = excluded.name, goal = excluded.goal,
                     rules = excluded.rules, max_active = excluded.max_active",
                params![
                    g.id.to_string(),
                    g.project_id.to_string(),
                    g.name,
                    g.goal,
                    json(&g.rules),
                    g.max_active.map(|n| n as i64)
                ],
            )?;
        }
        Op::SaveAgent(a) => {
            db.execute(
                "INSERT INTO agents (id, group_id, name, task, provider, worktree, rules, persona, model, color, position)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, (SELECT COUNT(*) FROM agents))
                 ON CONFLICT(id) DO UPDATE SET group_id = excluded.group_id, name = excluded.name,
                     task = excluded.task, provider = excluded.provider, rules = excluded.rules,
                     persona = excluded.persona, model = excluded.model, color = excluded.color",
                params![
                    a.id.to_string(),
                    a.group_id.to_string(),
                    a.name,
                    a.task,
                    json(&a.provider),
                    a.worktree.to_string_lossy(),
                    json(&a.rules),
                    a.persona,
                    a.model,
                    a.color
                ],
            )?;
        }
        Op::DeleteAgent(id) => {
            db.execute("DELETE FROM agents WHERE id = ?1", params![id.to_string()])?;
        }
        Op::SaveLayout { project, layout } => {
            db.execute("UPDATE projects SET layout = ?2 WHERE id = ?1", params![project.to_string(), layout])?;
        }
        Op::History(h) => {
            db.execute(
                "INSERT INTO history (agent_id, tool, summary, decision, source, decided_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![h.agent_id.to_string(), h.tool, h.summary, h.decision, h.source, Utc::now().to_rfc3339()],
            )?;
        }
        Op::SetSetting { key, value } => {
            db.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
        }
        Op::Setting { key, reply } => {
            let value =
                db.query_row("SELECT value FROM settings WHERE key = ?1", params![key], |row| row.get(0)).optional()?;
            let _ = reply.send(value);
        }
        Op::Load { root, reply } => {
            let _ = reply.send(load(db, &root)?);
        }
        Op::Opened(project) => {
            db.execute("UPDATE projects SET opened_at = unixepoch() WHERE id = ?1", params![project.to_string()])?;
        }
        Op::Recent { limit, reply } => {
            let mut statement = db.prepare("SELECT root FROM projects ORDER BY opened_at DESC, rowid DESC LIMIT ?1")?;
            let roots = statement
                .query_map(params![i64::try_from(limit).unwrap_or(i64::MAX)], |row| row.get::<_, String>(0))?
                .filter_map(Result::ok)
                .map(PathBuf::from)
                .collect();
            let _ = reply.send(roots);
        }
    }
    Ok(())
}

fn uuid(text: &str) -> Uuid {
    Uuid::parse_str(text).unwrap_or_default()
}

fn load(db: &Connection, root: &Path) -> rusqlite::Result<Option<LoadedProject>> {
    let project = db
        .query_row(
            "SELECT id, name, root, layout FROM projects WHERE root = ?1",
            params![root.to_string_lossy()],
            |row| {
                Ok((
                    Project {
                        id: uuid(&row.get::<_, String>(0)?),
                        name: row.get(1)?,
                        root: PathBuf::from(row.get::<_, String>(2)?),
                    },
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((project, layout)) = project else {
        return Ok(None);
    };

    let mut statement = db.prepare(
        "SELECT id, name, goal, rules, max_active FROM groups WHERE project_id = ?1 ORDER BY position, rowid",
    )?;
    let groups: Vec<Group> = statement
        .query_map(params![project.id.to_string()], |row| {
            Ok(Group {
                id: uuid(&row.get::<_, String>(0)?),
                project_id: project.id,
                name: row.get(1)?,
                goal: row.get(2)?,
                rules: serde_json::from_str::<Vec<PermissionRule>>(&row.get::<_, String>(3)?).unwrap_or_default(),
                max_active: row.get::<_, Option<i64>>(4)?.map(|n| n as usize),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;

    let mut agents = Vec::new();
    let mut statement = db.prepare(
        "SELECT id, group_id, name, task, provider, worktree, rules, persona, model, color FROM agents
         WHERE group_id = ?1 ORDER BY position, rowid",
    )?;
    for group in &groups {
        let rows = statement.query_map(params![group.id.to_string()], |row| {
            let provider: String = row.get(4)?;
            let rules: String = row.get(6)?;
            Ok(Agent {
                id: uuid(&row.get::<_, String>(0)?),
                group_id: uuid(&row.get::<_, String>(1)?),
                name: row.get(2)?,
                task: row.get(3)?,
                provider: serde_json::from_str::<Provider>(&provider)
                    .unwrap_or(Provider::CliAcp { command: "unknown".into() }),
                worktree: PathBuf::from(row.get::<_, String>(5)?),
                state: AgentState::Stopped,
                rules: serde_json::from_str(&rules).unwrap_or_default(),
                started_at: None,
                usage: Default::default(),
                persona: row.get(7)?,
                model: row.get(8)?,
                models: Vec::new(),
                color: row.get(9)?,
            })
        })?;
        for agent in rows {
            agents.push(agent?);
        }
    }
    Ok(Some(LoadedProject { project, groups, agents, layout }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Effect;

    #[tokio::test]
    async fn project_groups_agents_and_layout_survive_a_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("orch.db");
        let root = dir.path().join("shop");
        let project = Project { id: Uuid::new_v4(), name: "Shop".into(), root: root.clone() };
        let group = Group {
            id: Uuid::new_v4(),
            project_id: project.id,
            name: "API".into(),
            goal: "Pix".into(),
            rules: vec![PermissionRule::parse("Bash(cargo test:*)", Effect::Allow).unwrap()],
            max_active: Some(2),
        };
        let agent = Agent {
            id: Uuid::new_v4(),
            group_id: group.id,
            name: "Tests".into(),
            task: "write tests".into(),
            provider: Provider::Api {
                service: "anthropic".into(),
                model: "m".into(),
                key_ref: "orchestrator/anthropic".into(),
            },
            worktree: root.join(".worktrees/tests-1"),
            state: AgentState::Running,
            rules: Vec::new(),
            started_at: None,
            usage: Default::default(),
            persona: String::new(),
            model: Some("opus".into()),
            models: Vec::new(),
            color: None,
        };
        {
            let store = Store::open(&path).unwrap();
            store.save_project(&project);
            store.save_group(&group);
            store.save_agent(&agent);
            store.save_layout(project.id, "{\"view\":\"log\"}".into());
            store.set_setting("last_project", &root.to_string_lossy());
            assert_eq!(store.setting("last_project").await.as_deref(), Some(&*root.to_string_lossy()));
        }

        let store = Store::open(&path).unwrap();
        let loaded = store.load(&root).await.unwrap();
        assert_eq!(loaded.project, project);
        assert_eq!(loaded.groups, vec![group]);
        assert_eq!(loaded.agents.len(), 1);
        assert_eq!(loaded.agents[0].state, AgentState::Stopped);
        assert_eq!(loaded.agents[0].provider, agent.provider);
        assert_eq!(loaded.agents[0].model.as_deref(), Some("opus"));
        assert_eq!(loaded.layout.as_deref(), Some("{\"view\":\"log\"}"));
    }
}
