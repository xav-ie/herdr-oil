//! Turns an edited outline into the changes that make herdr match it.
//!
//! Numbers in the outline stand for the workspaces and tabs of the snapshot it
//! was rendered from (`before`), but the session keeps running while the editor is open, so every change is
//! resolved against a second snapshot taken after the editor closes (`now`).
//! Items that disappeared in between are skipped with a warning, and tabs that
//! appeared in between are left alone.
//!
//! Nothing in an outline is refused. A number that stands for nothing, or
//! that an earlier line already took (a copied line), makes its line a new
//! one, and a workspace left without tabs gets a new one.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::doc::Doc;
use crate::model::{Snapshot, Tab};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    /// In the order the outline lists them.
    pub workspaces: Vec<PlannedWorkspace>,
    /// Deleted tabs whose workspace stays.
    pub close_tabs: Vec<Tab>,
    /// Deleted workspaces, with every tab that dies with them.
    pub close_workspaces: Vec<ClosingWorkspace>,
    pub warnings: Vec<String>,
    /// The current name of every workspace and tab, by id, for describing
    /// what was done.
    #[serde(default)]
    pub names: BTreeMap<String, String>,
    /// The pane the editor was opened from, focused again once everything is
    /// done, wherever its tab went.
    #[serde(default)]
    pub focus: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlannedWorkspace {
    /// `None` for a workspace to create.
    pub id: Option<String>,
    /// A label to set; `None` keeps the current one (or herdr's default).
    pub label: Option<String>,
    pub tabs: Vec<PlannedTab>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlannedTab {
    pub source: TabSource,
    /// A label to set; `None` keeps the current one (or herdr's default).
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum TabSource {
    New,
    Existing {
        id: String,
        /// Where the tab is now, which may differ from the planned workspace.
        workspace_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClosingWorkspace {
    pub id: String,
    pub label: String,
    pub tabs: Vec<Tab>,
}

impl Plan {
    #[must_use]
    pub fn closes_anything(&self) -> bool {
        !self.close_tabs.is_empty() || !self.close_workspaces.is_empty()
    }
}

#[must_use]
pub fn plan(before: &Snapshot, doc: &Doc, now: &Snapshot) -> Plan {
    let outline = resolve(before, doc);
    let mut plan = Plan {
        names: now
            .workspaces
            .iter()
            .flat_map(|w| {
                std::iter::once((w.id.clone(), w.label.clone()))
                    .chain(w.tabs.iter().map(|t| (t.id.clone(), t.label.clone())))
            })
            .collect(),
        ..Plan::default()
    };
    let kept_tabs: HashSet<&str> = outline
        .iter()
        .flat_map(|w| &w.tabs)
        .filter_map(|t| t.id.as_deref())
        .collect();
    let kept_workspaces: HashSet<&str> = outline.iter().filter_map(|w| w.id.as_deref()).collect();

    for doc_workspace in &outline {
        let label = doc_workspace.label;
        let (id, label) = match doc_workspace.id.as_deref() {
            Some(id) if now.workspace(id).is_some() => {
                let old = &before.workspace(id).expect("resolved").label;
                (Some(id.to_owned()), changed(old, label))
            }
            Some(id) => {
                let old = &before.workspace(id).expect("resolved").label;
                plan.warnings.push(format!(
                    "workspace \"{old}\" closed while you were editing; creating \"{label}\" instead"
                ));
                (None, non_empty(label))
            }
            None => (None, non_empty(label)),
        };

        let mut tabs = Vec::new();
        for doc_tab in &doc_workspace.tabs {
            let Some(tab_id) = doc_tab.id.as_deref() else {
                tabs.push(PlannedTab {
                    source: TabSource::New,
                    label: non_empty(doc_tab.label),
                });
                continue;
            };
            let (_, old, position) = before.tab(tab_id).expect("resolved");
            let Some((now_workspace, _, _)) = now.tab(tab_id) else {
                plan.warnings.push(format!(
                    "tab \"{}\" closed while you were editing; skipped",
                    old.label
                ));
                continue;
            };
            let moves = id.as_deref() != Some(now_workspace.id.as_str());
            let label = if changed(&old.label, doc_tab.label).is_some() {
                non_empty(doc_tab.label)
            } else if moves && !old.has_automatic_label(position) {
                // A moved tab lands in a new tab, which needs its name given again.
                Some(old.label.clone())
            } else {
                None
            };
            tabs.push(PlannedTab {
                source: TabSource::Existing {
                    id: tab_id.to_owned(),
                    workspace_id: now_workspace.id.clone(),
                },
                label,
            });
        }

        if tabs.is_empty() && id.is_none() {
            plan.warnings.push(format!(
                "every tab of new workspace \"{label}\" is gone; not creating it",
                label = doc_workspace.label
            ));
            continue;
        }
        plan.workspaces.push(PlannedWorkspace { id, label, tabs });
    }

    let deleted_before =
        |tab: &Tab| before.tab(&tab.id).is_some() && !kept_tabs.contains(tab.id.as_str());
    for workspace in &now.workspaces {
        let closing = before.workspace(&workspace.id).is_some()
            && !kept_workspaces.contains(workspace.id.as_str());
        if closing {
            // Everything still here when the workspace closes dies with it,
            // including tabs opened while the editor was up.
            plan.close_workspaces.push(ClosingWorkspace {
                id: workspace.id.clone(),
                label: workspace.label.clone(),
                tabs: workspace
                    .tabs
                    .iter()
                    .filter(|t| !kept_tabs.contains(t.id.as_str()))
                    .cloned()
                    .collect(),
            });
        } else {
            plan.close_tabs
                .extend(workspace.tabs.iter().filter(|t| deleted_before(t)).cloned());
        }
    }

    plan
}

struct Resolved<'a> {
    id: Option<String>,
    label: &'a str,
    tabs: Vec<ResolvedTab<'a>>,
}

struct ResolvedTab<'a> {
    id: Option<String>,
    label: &'a str,
}

/// The outline with its numbers turned into the ids they stood for in
/// `before`.
fn resolve<'a>(before: &Snapshot, doc: &'a Doc) -> Vec<Resolved<'a>> {
    let before_tabs: Vec<&Tab> = before.workspaces.iter().flat_map(|w| &w.tabs).collect();
    // Workspace and tab ids never collide, so one set serves both.
    let mut taken: HashSet<&str> = HashSet::new();
    let mut outline = Vec::new();
    for workspace in &doc.workspaces {
        let id = workspace
            .number
            .and_then(|n| before.workspaces.get(n - 1))
            .map(|w| w.id.as_str())
            .filter(|id| taken.insert(id))
            .map(str::to_owned);
        let mut tabs = Vec::new();
        for tab in &workspace.tabs {
            let id = tab
                .number
                .and_then(|n| before_tabs.get(n - 1))
                .map(|t| t.id.as_str())
                .filter(|id| taken.insert(id))
                .map(str::to_owned);
            tabs.push(ResolvedTab {
                id,
                label: &tab.label,
            });
        }
        if tabs.is_empty() {
            tabs.push(ResolvedTab {
                id: None,
                label: "",
            });
        }
        outline.push(Resolved {
            id,
            label: &workspace.label,
            tabs,
        });
    }
    outline
}

fn non_empty(label: &str) -> Option<String> {
    (!label.is_empty()).then(|| label.to_owned())
}

/// The outline trims names, so a name herdr has with spaces around it is
/// unchanged when only those spaces are missing.
fn changed(old: &str, new: &str) -> Option<String> {
    (old.trim() != new).then(|| non_empty(new)).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::parse;
    use crate::model::Workspace;

    fn tab(id: &str, label: &str) -> Tab {
        Tab {
            id: id.into(),
            label: label.into(),
            pane_count: 1,
            agents: Vec::new(),
        }
    }

    fn session() -> Snapshot {
        Snapshot {
            workspaces: vec![
                Workspace {
                    id: "w1".into(),
                    label: "dots".into(),
                    tabs: vec![
                        tab("w1:t1", "nix"),
                        tab("w1:t2", "2"),
                        tab("w1:t3", "voice"),
                    ],
                },
                Workspace {
                    id: "w2".into(),
                    label: "work".into(),
                    tabs: vec![tab("w2:t1", "review")],
                },
            ],
        }
    }

    fn plan_text(text: &str) -> Plan {
        plan(&session(), &parse(text).unwrap(), &session())
    }

    fn existing(id: &str, workspace_id: &str, label: Option<&str>) -> PlannedTab {
        PlannedTab {
            source: TabSource::Existing {
                id: id.into(),
                workspace_id: workspace_id.into(),
            },
            label: label.map(Into::into),
        }
    }

    #[test]
    fn unchanged_outline_changes_nothing() {
        let plan = plan(
            &session(),
            &parse(&crate::doc::render(&session())).unwrap(),
            &session(),
        );
        assert!(!plan.closes_anything());
        assert_eq!(plan.warnings, Vec::<String>::new());
        assert!(
            plan.workspaces
                .iter()
                .all(|w| w.label.is_none() && w.tabs.iter().all(|t| t.label.is_none()))
        );
    }

    #[test]
    fn moves_keep_custom_labels_and_drop_automatic_ones() {
        let plan =
            plan_text("/1 dots:\n  - /1 nix\n\n/2 work:\n  - /4 review\n  - /3 voice\n  - /2 2\n");
        assert_eq!(
            plan.workspaces[1].tabs,
            [
                existing("w2:t1", "w2", None),
                existing("w1:t3", "w1", Some("voice")),
                existing("w1:t2", "w1", None),
            ]
        );
    }

    #[test]
    fn renames_creates_and_closes() {
        let plan = plan_text(
            "/1 dotfiles:\n  - /1 nix flake\n  - scratch\n\nexperiments:\n  - /4 review\n",
        );

        assert_eq!(plan.workspaces[0].label.as_deref(), Some("dotfiles"));
        assert_eq!(
            plan.workspaces[0].tabs,
            [
                existing("w1:t1", "w1", Some("nix flake")),
                PlannedTab {
                    source: TabSource::New,
                    label: Some("scratch".into())
                },
            ]
        );
        assert_eq!(plan.workspaces[1].id, None);
        assert_eq!(
            plan.workspaces[1].tabs,
            [existing("w2:t1", "w2", Some("review"))]
        );

        let closed: Vec<&str> = plan.close_tabs.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(closed, ["w1:t2", "w1:t3"]);
        assert_eq!(plan.close_workspaces[0].id, "w2");
        assert_eq!(plan.close_workspaces[0].tabs, []);
    }

    #[test]
    fn deleting_a_workspace_line_closes_its_remaining_tabs_with_it() {
        let plan = plan_text("/1 dots:\n  - /1 nix\n  - /2 2\n  - /3 voice\n");
        assert_eq!(plan.close_tabs, []);
        let closing = &plan.close_workspaces[0];
        assert_eq!((closing.id.as_str(), closing.tabs.len()), ("w2", 1));
    }

    #[test]
    fn unknown_copied_and_emptied_lines_become_new() {
        // `/1` twice (a copied line), `/9` stands for nothing, `/2` work is
        // left without tabs, and `/1` dots is listed again further down.
        let plan = plan_text(
            "/1 dots:\n  - /1 nix\n  - /1 nix copy\n  - /9 ghost\n  - /2 2\n  - /3 voice\n/2 work:\n/1 again:\n  - /4 review\n",
        );
        let new = |label: &str| PlannedTab {
            source: TabSource::New,
            label: Some(label.into()),
        };
        assert_eq!(
            plan.workspaces[0].tabs,
            [
                existing("w1:t1", "w1", None),
                new("nix copy"),
                new("ghost"),
                existing("w1:t2", "w1", None),
                existing("w1:t3", "w1", None),
            ]
        );
        assert_eq!(
            plan.workspaces[1],
            PlannedWorkspace {
                id: Some("w2".into()),
                label: None,
                tabs: vec![PlannedTab {
                    source: TabSource::New,
                    label: None
                }],
            }
        );
        assert_eq!(
            (
                plan.workspaces[2].id.as_deref(),
                plan.workspaces[2].label.as_deref()
            ),
            (None, Some("again"))
        );
        assert!(!plan.closes_anything());
    }

    #[test]
    fn changes_made_while_editing_are_respected() {
        let mut now = session();
        // w1:t3 closed, w2 gained a tab, and w1:t1 was moved to w2, which
        // gives it a new id there.
        let mut moved = now.workspaces[0].tabs.remove(0);
        moved.id = "w2:t6".into();
        now.workspaces[0].tabs.pop();
        now.workspaces[1].tabs.push(tab("w2:t5", "fresh"));
        now.workspaces[1].tabs.push(moved);

        let doc = parse("/1 dots:\n  - /1 nix\n  - /2 2\n  - /3 voice\n").unwrap();
        let plan = plan(&session(), &doc, &now);

        assert_eq!(plan.warnings.len(), 2, "{:?}", plan.warnings);
        assert_eq!(plan.workspaces[0].tabs, [existing("w1:t2", "w1", None)]);
        let dying: Vec<&str> = plan.close_workspaces[0]
            .tabs
            .iter()
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(dying, ["w2:t1", "w2:t5", "w2:t6"]);
    }

    #[test]
    fn names_with_spaces_around_them_are_not_renamed_by_the_trimmed_outline() {
        let mut session = session();
        session.workspaces[0].label = " dots ".into();
        session.workspaces[0].tabs[0].label = "  nix".into();
        let doc = parse(&crate::doc::render(&session)).unwrap();
        let plan = plan(&session, &doc, &session);
        assert_eq!(plan.workspaces[0].label, None);
        assert_eq!(plan.workspaces[0].tabs[0].label, None);
    }
}
