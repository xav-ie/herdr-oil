//! The part of herdr's session this plugin edits: workspaces and their tabs, in
//! sidebar order.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::rpc::Herdr;

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub workspaces: Vec<Workspace>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Workspace {
    pub id: String,
    pub label: String,
    pub tabs: Vec<Tab>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tab {
    pub id: String,
    pub label: String,
    pub pane_count: u64,
    /// Agents running in the tab's panes, by display name.
    pub agents: Vec<String>,
}

impl Tab {
    /// herdr labels an unnamed tab with its 1-based position and renumbers it
    /// as tabs move, so such a label is not worth keeping on purpose.
    #[must_use]
    pub fn has_automatic_label(&self, position: usize) -> bool {
        self.label == position.to_string()
    }
}

impl Snapshot {
    pub fn fetch(herdr: &mut impl Herdr) -> Result<Self> {
        let result = herdr.call("session.snapshot", json!({}))?;
        Self::from_json(&result["snapshot"])
    }

    pub fn from_json(snapshot: &Value) -> Result<Self> {
        let list = |key: &str| {
            snapshot[key]
                .as_array()
                .with_context(|| format!("session snapshot has no {key}"))
        };
        let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();

        let agents = list("agents")?;
        let mut workspaces: Vec<Workspace> = list("workspaces")?
            .iter()
            .map(|w| Workspace {
                id: text(&w["workspace_id"]),
                label: text(&w["label"]),
                tabs: Vec::new(),
            })
            .collect();

        for t in list("tabs")? {
            let id = text(&t["tab_id"]);
            let tab = Tab {
                agents: agents
                    .iter()
                    .filter(|a| a["tab_id"].as_str() == Some(&id))
                    .filter_map(|a| a["display_agent"].as_str().or(a["agent"].as_str()))
                    .map(str::to_owned)
                    .collect(),
                label: text(&t["label"]),
                pane_count: t["pane_count"].as_u64().unwrap_or(1),
                id,
            };
            if let Some(workspace) = workspaces
                .iter_mut()
                .find(|w| t["workspace_id"].as_str() == Some(&w.id))
            {
                workspace.tabs.push(tab);
            }
        }
        Ok(Self { workspaces })
    }

    #[must_use]
    pub fn workspace(&self, id: &str) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.id == id)
    }

    /// The tab, its workspace, and its 1-based position there.
    #[must_use]
    pub fn tab(&self, id: &str) -> Option<(&Workspace, &Tab, usize)> {
        self.workspaces.iter().find_map(|w| {
            w.tabs
                .iter()
                .position(|t| t.id == id)
                .map(|i| (w, &w.tabs[i], i + 1))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_tabs_under_workspaces_in_order() {
        let snapshot = Snapshot::from_json(&json!({
            "workspaces": [
                { "workspace_id": "w1", "label": "dots" },
                { "workspace_id": "w2", "label": "work" },
            ],
            "tabs": [
                { "tab_id": "w2:t1", "workspace_id": "w2", "label": "review", "pane_count": 1 },
                { "tab_id": "w1:t2", "workspace_id": "w1", "label": "nix", "pane_count": 2 },
                { "tab_id": "w1:t1", "workspace_id": "w1", "label": "2", "pane_count": 1 },
            ],
            "agents": [
                { "tab_id": "w1:t2", "agent": "claude", "display_agent": "Claude" },
                { "tab_id": "w1:t2", "agent": "codex" },
            ],
        }))
        .unwrap();

        let ids: Vec<Vec<&str>> = snapshot
            .workspaces
            .iter()
            .map(|w| w.tabs.iter().map(|t| t.id.as_str()).collect())
            .collect();
        assert_eq!(ids, [vec!["w1:t2", "w1:t1"], vec!["w2:t1"]]);
        assert_eq!(snapshot.workspaces[0].tabs[0].agents, ["Claude", "codex"]);

        let (workspace, tab, position) = snapshot.tab("w1:t1").unwrap();
        assert_eq!((workspace.id.as_str(), position), ("w1", 2));
        assert!(tab.has_automatic_label(position));
    }
}
