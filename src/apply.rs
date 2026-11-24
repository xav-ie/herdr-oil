//! Carries out a [`Plan`] through herdr's API.
//!
//! Order matters because herdr closes a workspace the moment its last tab
//! leaves: new tabs are created before anything moves out, and closing comes
//! last. A failed step is reported and the rest still run, so one stale id
//! doesn't strand a half-applied outline.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::layout::Node;
use crate::plan::{Plan, PlannedTab, TabSource};
use crate::rpc::Herdr;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Report {
    pub done: Vec<String>,
    pub errors: Vec<String>,
}

impl Report {
    fn record<T>(&mut self, result: Result<T>, describe: impl FnOnce() -> String) -> Option<T> {
        match result {
            Ok(value) => {
                self.done.push(describe());
                Some(value)
            }
            Err(error) => {
                self.errors.push(format!("{}: {error:#}", describe()));
                None
            }
        }
    }

    /// Like [`Report::record`] for reads, which only matter when they fail.
    fn check<T>(&mut self, result: Result<T>, describe: impl FnOnce() -> String) -> Option<T> {
        result
            .map_err(|error| self.errors.push(format!("{}: {error:#}", describe())))
            .ok()
    }
}

pub fn apply(plan: &Plan, herdr: &mut impl Herdr) -> Report {
    let mut run = Run {
        plan,
        herdr,
        report: Report::default(),
        workspace_ids: plan.workspaces.iter().map(|w| w.id.clone()).collect(),
        tab_ids: plan
            .workspaces
            .iter()
            .map(|w| vec![None; w.tabs.len()])
            .collect(),
        names: plan.names.clone().into_iter().collect(),
        pane_ids: HashMap::new(),
    };
    run.create_tabs_in_existing_workspaces();
    run.create_workspaces();
    run.place_tabs();
    run.rename_workspaces();
    run.close();
    run.reorder_workspaces();
    run.reorder_tabs();
    run.focus();
    run.report
}

struct Run<'a, H> {
    plan: &'a Plan,
    herdr: &'a mut H,
    report: Report,
    /// Ids as they become known, by outline position.
    workspace_ids: Vec<Option<String>>,
    tab_ids: Vec<Vec<Option<String>>>,
    /// Names by id, for describing steps; ids made along the way are added.
    names: HashMap<String, String>,
    /// Where each moved pane went, by its old id.
    pane_ids: HashMap<String, String>,
}

impl<H: Herdr> Run<'_, H> {
    fn name(&self, id: &str) -> String {
        self.names
            .get(id)
            .map_or_else(|| format!("/{id}"), |name| format!("\"{name}\""))
    }

    /// Notes a moved tab's new ids, and its name under the new id.
    fn moved(&mut self, from: &str, to: Moved) -> (String, String) {
        let name = self.names.get(from).cloned();
        if let Some(name) = name {
            self.names.insert(to.tab_id.clone(), name);
        }
        self.pane_ids.extend(to.panes);
        (to.workspace_id, to.tab_id)
    }

    fn create_tabs_in_existing_workspaces(&mut self) {
        for (w, workspace) in self.plan.workspaces.iter().enumerate() {
            let Some(workspace_id) = &workspace.id else {
                continue;
            };
            for (t, tab) in workspace.tabs.iter().enumerate() {
                if tab.source == TabSource::New {
                    self.tab_ids[w][t] = self.create_tab(workspace_id, tab);
                }
            }
        }
    }

    /// Each new workspace is made from its first tab: created fresh, or the
    /// existing tab moved into it.
    fn create_workspaces(&mut self) {
        for (w, workspace) in self.plan.workspaces.iter().enumerate() {
            if workspace.id.is_some() {
                continue;
            }
            let first = &workspace.tabs[0];
            let name = workspace.label.as_deref().unwrap_or("new workspace");
            let created = match &first.source {
                TabSource::New => {
                    let created =
                        self.create_workspace(workspace.label.as_deref(), first.label.as_deref());
                    self.report
                        .record(created, || format!("create workspace \"{name}\""))
                }
                TabSource::Existing { id, .. } => {
                    let destination = json!({
                        "type": "new_workspace",
                        "label": workspace.label,
                        "tab_label": first.label,
                    });
                    let moved = move_tab(self.herdr, id, &destination);
                    let tab = self.name(id);
                    self.report
                        .record(moved, || format!("move {tab} to new workspace \"{name}\""))
                        .map(|moved| self.moved(id, moved))
                }
            };
            if let Some((workspace_id, tab_id)) = created {
                self.names.insert(workspace_id.clone(), name.to_owned());
                self.workspace_ids[w] = Some(workspace_id);
                self.tab_ids[w][0] = Some(tab_id);
            }
        }
    }

    /// Moves tabs between workspaces, renames tabs that stay, and creates the
    /// rest of the new workspaces' tabs.
    fn place_tabs(&mut self) {
        for (w, workspace) in self.plan.workspaces.iter().enumerate() {
            let Some(workspace_id) = self.workspace_ids[w].clone() else {
                continue;
            };
            for (t, tab) in workspace.tabs.iter().enumerate() {
                if self.tab_ids[w][t].is_none() {
                    self.tab_ids[w][t] = self.place_tab(&workspace_id, tab);
                }
            }
        }
    }

    fn place_tab(&mut self, workspace_id: &str, tab: &PlannedTab) -> Option<String> {
        match &tab.source {
            TabSource::New => self.create_tab(workspace_id, tab),
            TabSource::Existing {
                id,
                workspace_id: from,
            } if from == workspace_id => {
                if let Some(label) = &tab.label {
                    let renamed = self
                        .herdr
                        .call("tab.rename", json!({ "tab_id": id, "label": label }));
                    let old = self.name(id);
                    self.report
                        .record(renamed, || format!("rename tab {old} to \"{label}\""));
                    self.names.insert(id.clone(), label.clone());
                }
                Some(id.clone())
            }
            TabSource::Existing { id, .. } => {
                let destination = json!({
                    "type": "new_tab",
                    "workspace_id": workspace_id,
                    "label": tab.label,
                });
                let moved = move_tab(self.herdr, id, &destination);
                let (tab_name, to) = (self.name(id), self.name(workspace_id));
                self.report
                    .record(moved, || format!("move {tab_name} to {to}"))
                    .map(|moved| self.moved(id, moved).1)
            }
        }
    }

    fn rename_workspaces(&mut self) {
        let plan = self.plan;
        for workspace in &plan.workspaces {
            if let (Some(id), Some(label)) = (&workspace.id, &workspace.label) {
                let renamed = self.herdr.call(
                    "workspace.rename",
                    json!({ "workspace_id": id, "label": label }),
                );
                let old = self.name(id);
                self.report
                    .record(renamed, || format!("rename workspace {old} to \"{label}\""));
            }
        }
    }

    fn close(&mut self) {
        for tab in &self.plan.close_tabs {
            let closed = self.herdr.call("tab.close", json!({ "tab_id": tab.id }));
            self.report
                .record(closed, || format!("close tab \"{}\"", tab.label));
        }
        if self.plan.close_workspaces.is_empty() {
            return;
        }
        // A workspace whose tabs all moved out is already gone.
        let listed = list_ids(
            self.herdr,
            "workspace.list",
            &json!({}),
            "workspaces",
            "workspace_id",
        );
        let Some(open) = self.report.check(listed, || "list workspaces".into()) else {
            return;
        };
        for workspace in &self.plan.close_workspaces {
            if open.contains(&workspace.id) {
                let closed = self
                    .herdr
                    .call("workspace.close", json!({ "workspace_id": workspace.id }));
                self.report.record(closed, || {
                    format!("close workspace \"{}\"", workspace.label)
                });
            }
        }
    }

    fn reorder_workspaces(&mut self) {
        let desired: Vec<String> = self.workspace_ids.iter().flatten().cloned().collect();
        let listed = list_ids(
            self.herdr,
            "workspace.list",
            &json!({}),
            "workspaces",
            "workspace_id",
        );
        let Some(current) = self.report.check(listed, || "read workspace order".into()) else {
            return;
        };
        for (id, index) in reorder(current, &desired) {
            let moved = self.herdr.call(
                "workspace.move",
                json!({ "workspace_id": id, "insert_index": index }),
            );
            let name = self.name(&id);
            self.report.record(moved, || {
                format!("move workspace {name} to position {}", index + 1)
            });
        }
    }

    fn reorder_tabs(&mut self) {
        for w in 0..self.workspace_ids.len() {
            let Some(workspace_id) = self.workspace_ids[w].clone() else {
                continue;
            };
            let desired: Vec<String> = self.tab_ids[w].iter().flatten().cloned().collect();
            let params = json!({ "workspace_id": workspace_id });
            let listed = list_ids(self.herdr, "tab.list", &params, "tabs", "tab_id");
            let Some(current) = self
                .report
                .check(listed, || format!("read tab order of /{workspace_id}"))
            else {
                continue;
            };
            for (id, index) in reorder(current, &desired) {
                let moved = self
                    .herdr
                    .call("tab.move", json!({ "tab_id": id, "insert_index": index }));
                let (name, within) = (self.name(&id), self.name(&workspace_id));
                self.report.record(moved, || {
                    format!("move {name} to position {} in {within}", index + 1)
                });
            }
        }
    }

    fn create_tab(&mut self, workspace_id: &str, tab: &PlannedTab) -> Option<String> {
        let created = self
            .herdr
            .call(
                "tab.create",
                json!({ "workspace_id": workspace_id, "label": tab.label, "focus": false }),
            )
            .and_then(|result| string(&result["tab"]["tab_id"]));
        let name = tab.label.as_deref().unwrap_or("new tab");
        let within = self.name(workspace_id);
        let created = self
            .report
            .record(created, || format!("create tab \"{name}\" in {within}"));
        if let Some(id) = &created {
            self.names.insert(id.clone(), name.to_owned());
        }
        created
    }

    /// Focuses the last new tab in the outline, as a browser does with a tab
    /// you open, or else the pane the editor was opened from, wherever its tab
    /// went, so moves happen around you instead of taking you elsewhere.
    fn focus(&mut self) {
        let newest = self
            .plan
            .workspaces
            .iter()
            .zip(&self.tab_ids)
            .flat_map(|(workspace, ids)| workspace.tabs.iter().zip(ids))
            .filter(|(tab, _)| tab.source == TabSource::New)
            .filter_map(|(_, id)| id.clone())
            .next_back();
        if let Some(tab) = newest {
            let focused = self.herdr.call("tab.focus", json!({ "tab_id": tab }));
            let name = self.name(&tab);
            self.report.check(focused, || format!("focus {name}"));
            return;
        }
        let Some(pane) = &self.plan.focus else {
            return;
        };
        let pane = self.pane_ids.get(pane).unwrap_or(pane);
        // A pane that is gone was closed on purpose; nothing to return to.
        let _ = self.herdr.call("pane.focus", json!({ "pane_id": pane }));
    }

    /// Creates a workspace and names its first tab, returning both ids.
    fn create_workspace(
        &mut self,
        label: Option<&str>,
        tab_label: Option<&str>,
    ) -> Result<(String, String)> {
        let result = self.herdr.call(
            "workspace.create",
            json!({ "label": label, "focus": false }),
        )?;
        let workspace_id = string(&result["workspace"]["workspace_id"])?;
        let tab_id = string(&result["tab"]["tab_id"])?;
        if let Some(label) = tab_label {
            self.herdr
                .call("tab.rename", json!({ "tab_id": tab_id, "label": label }))?;
        }
        Ok((workspace_id, tab_id))
    }
}

/// Where [`move_tab`] put a tab.
pub struct Moved {
    pub workspace_id: String,
    pub tab_id: String,
    /// The panes' new ids, by their old ones.
    pub panes: HashMap<String, String>,
}

/// Moves every pane of `tab_id` to `destination`, a `pane.move` destination
/// that makes a new tab, keeping the split layout and zoom.
pub fn move_tab(herdr: &mut impl Herdr, tab_id: &str, destination: &Value) -> Result<Moved> {
    let export = herdr.call("layout.export", json!({ "tab_id": tab_id }))?;
    let layout = &export["layout"];
    let root = Node::from_export(&layout["root"])?;

    // herdr won't move a pane out of a zoomed tab, so unzoom it here and zoom
    // the same pane again once the tab is rebuilt. The zoomed pane is the
    // focused one.
    let zoomed = if layout["zoomed"].as_bool() == Some(true) {
        let pane = string(&layout["focused_pane_id"])?;
        herdr.call("pane.zoom", json!({ "pane_id": pane, "mode": "off" }))?;
        Some(pane)
    } else {
        None
    };

    // Pane ids change when a pane changes workspace.
    let mut moved: HashMap<String, String> = HashMap::new();
    let mut landed: Option<(String, String)> = None;
    for step in root.steps() {
        let destination = match (&step.split, &landed) {
            (None, _) => destination.clone(),
            (Some(split), Some((_, tab))) => json!({
                "type": "tab",
                "tab_id": tab,
                "target_pane_id": moved[&split.anchor],
                "split": split.direction,
                "ratio": split.ratio,
            }),
            (Some(_), None) => unreachable!("the first step has no split"),
        };
        let result = herdr.call(
            "pane.move",
            json!({ "pane_id": step.pane, "destination": destination, "focus": false }),
        )?;
        let result = &result["move_result"];
        if result["changed"].as_bool() == Some(false) {
            bail!(
                "herdr did not move pane /{}: {}",
                step.pane,
                result["reason"].as_str().unwrap_or("no reason given")
            );
        }
        let pane = &result["pane"];
        moved.insert(step.pane.clone(), string(&pane["pane_id"])?);
        if landed.is_none() {
            landed = Some((string(&pane["workspace_id"])?, string(&pane["tab_id"])?));
        }
    }
    if let Some(pane) = zoomed {
        herdr.call(
            "pane.zoom",
            json!({ "pane_id": moved[&pane], "mode": "on" }),
        )?;
    }
    let (workspace_id, tab_id) = landed.context("tab has no panes")?;
    Ok(Moved {
        workspace_id,
        tab_id,
        panes: moved,
    })
}

fn list_ids(
    herdr: &mut impl Herdr,
    method: &str,
    params: &Value,
    key: &str,
    id: &str,
) -> Result<Vec<String>> {
    let result = herdr.call(method, params.clone())?;
    result[key]
        .as_array()
        .with_context(|| format!("{method} returned no {key}"))?
        .iter()
        .map(|item| string(&item[id]))
        .collect()
}

fn string(value: &Value) -> Result<String> {
    value
        .as_str()
        .map(str::to_owned)
        .with_context(|| format!("expected an id, got {value}"))
}

/// The `(id, index)` moves that put `desired` first, in order, ahead of any
/// ids only `current` has. Ids `current` lacks are ignored.
fn reorder(mut current: Vec<String>, desired: &[String]) -> Vec<(String, usize)> {
    let mut moves = Vec::new();
    let present: Vec<&String> = desired.iter().filter(|id| current.contains(id)).collect();
    for (index, id) in present.into_iter().enumerate() {
        let from = current.iter().position(|c| c == id).expect("filtered");
        if from != index {
            let id = current.remove(from);
            current.insert(index, id.clone());
            moves.push((id, index));
        }
    }
    moves
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Tab;
    use crate::plan::{ClosingWorkspace, PlannedWorkspace};
    use anyhow::anyhow;

    /// Answers with just enough shape for `apply`, records every call, and
    /// fails calls whose method and params contain a given fragment.
    #[derive(Default)]
    struct Fake {
        calls: Vec<(String, Value)>,
        fail: Option<(&'static str, &'static str)>,
        /// `pane.move` answers that nothing moved, as herdr does for a
        /// zoomed tab.
        refuse_moves: bool,
        workspaces: Vec<&'static str>,
        next: usize,
    }

    impl Herdr for Fake {
        fn call(&mut self, method: &str, params: Value) -> Result<Value> {
            self.calls.push((method.to_owned(), params.clone()));
            if let Some((m, fragment)) = self.fail
                && m == method
                && params.to_string().contains(fragment)
            {
                return Err(anyhow!("nope"));
            }
            self.next += 1;
            let n = self.next;
            Ok(match method {
                "tab.create" => json!({ "tab": { "tab_id": format!("wN:t{n}") } }),
                "workspace.create" => json!({
                    "workspace": { "workspace_id": format!("w{n}") },
                    "tab": { "tab_id": format!("w{n}:t1") },
                }),
                "layout.export" => {
                    json!({ "layout": { "root": { "type": "pane", "pane_id": "w1:p1" } } })
                }
                "pane.move" if self.refuse_moves => json!({ "move_result": {
                    "changed": false, "reason": "zoomed_tab",
                    "pane": { "pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1" },
                } }),
                "pane.move" => json!({ "move_result": { "pane": {
                    "pane_id": format!("w2:p{n}"), "workspace_id": "w2", "tab_id": format!("w2:t{n}"),
                } } }),
                "workspace.list" => json!({ "workspaces": self.workspaces.iter()
                    .map(|id| json!({ "workspace_id": id })).collect::<Vec<_>>() }),
                "tab.list" => json!({ "tabs": [] }),
                _ => json!({}),
            })
        }
    }

    impl Fake {
        fn methods(&self) -> Vec<&str> {
            self.calls.iter().map(|(m, _)| m.as_str()).collect()
        }
    }

    fn tab(id: &str) -> Tab {
        Tab {
            id: id.into(),
            label: id.into(),
            pane_count: 1,
            agents: Vec::new(),
        }
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
    fn new_tabs_exist_before_anything_moves_out() {
        // w1's only tab leaves for w2 while w1 gains a new one: had the move
        // run first, herdr would have closed w1 under us.
        let plan = Plan {
            workspaces: vec![
                PlannedWorkspace {
                    id: Some("w1".into()),
                    label: None,
                    tabs: vec![PlannedTab {
                        source: TabSource::New,
                        label: Some("fresh".into()),
                    }],
                },
                PlannedWorkspace {
                    id: Some("w2".into()),
                    label: None,
                    tabs: vec![existing("w1:t1", "w1", None)],
                },
            ],
            ..Plan::default()
        };
        let mut fake = Fake {
            workspaces: vec!["w1", "w2"],
            ..Fake::default()
        };
        let report = apply(&plan, &mut fake);
        assert_eq!(report.errors, Vec::<String>::new());

        let methods = fake.methods();
        let created = methods.iter().position(|m| *m == "tab.create").unwrap();
        let moved = methods.iter().position(|m| *m == "pane.move").unwrap();
        assert!(created < moved, "{methods:?}");
    }

    #[test]
    fn a_failed_step_is_reported_and_the_rest_still_run() {
        let plan = Plan {
            workspaces: vec![PlannedWorkspace {
                id: Some("w1".into()),
                label: Some("renamed".into()),
                tabs: vec![
                    existing("w1:t1", "w1", Some("bad")),
                    existing("w1:t2", "w1", Some("good")),
                ],
            }],
            close_tabs: vec![tab("w1:t3")],
            names: [("w1", "alpha"), ("w1:t1", "one"), ("w1:t2", "two")]
                .into_iter()
                .map(|(id, name)| (id.into(), name.into()))
                .collect(),
            ..Plan::default()
        };
        let mut fake = Fake {
            fail: Some(("tab.rename", "bad")),
            workspaces: vec!["w1"],
            ..Fake::default()
        };
        let report = apply(&plan, &mut fake);

        assert_eq!(report.errors.len(), 1);
        assert!(
            report.errors[0].starts_with("rename tab \"one\""),
            "{:?}",
            report.errors
        );
        assert_eq!(
            report.done,
            [
                "rename tab \"two\" to \"good\"",
                "rename workspace \"alpha\" to \"renamed\"",
                "close tab \"w1:t3\"",
            ]
        );
    }

    #[test]
    fn a_move_herdr_refuses_is_an_error_not_a_success() {
        let plan = Plan {
            workspaces: vec![PlannedWorkspace {
                id: Some("w2".into()),
                label: None,
                tabs: vec![existing("w1:t1", "w1", None)],
            }],
            ..Plan::default()
        };
        let mut fake = Fake {
            refuse_moves: true,
            workspaces: vec!["w2"],
            ..Fake::default()
        };
        let report = apply(&plan, &mut fake);
        assert_eq!(report.done, Vec::<String>::new());
        assert_eq!(report.errors.len(), 1);
        assert!(
            report.errors[0].contains("zoomed_tab"),
            "{:?}",
            report.errors
        );
    }

    #[test]
    fn workspaces_that_already_closed_are_not_closed_again() {
        let closing = |id: &str| ClosingWorkspace {
            id: id.into(),
            label: id.into(),
            tabs: Vec::new(),
        };
        let plan = Plan {
            close_workspaces: vec![closing("w1"), closing("w2")],
            ..Plan::default()
        };
        let mut fake = Fake {
            workspaces: vec!["w2"],
            ..Fake::default()
        };
        let report = apply(&plan, &mut fake);

        let closed: Vec<&Value> = fake
            .calls
            .iter()
            .filter(|(m, _)| m == "workspace.close")
            .map(|(_, p)| &p["workspace_id"])
            .collect();
        assert_eq!(closed, [&json!("w2")]);
        assert_eq!(report.errors, Vec::<String>::new());
    }

    #[test]
    fn a_failed_new_workspace_skips_its_tabs_instead_of_misplacing_them() {
        let plan = Plan {
            workspaces: vec![PlannedWorkspace {
                id: None,
                label: Some("gamma".into()),
                tabs: vec![
                    PlannedTab {
                        source: TabSource::New,
                        label: Some("one".into()),
                    },
                    existing("w1:t1", "w1", None),
                ],
            }],
            ..Plan::default()
        };
        let mut fake = Fake {
            fail: Some(("workspace.create", "gamma")),
            ..Fake::default()
        };
        let report = apply(&plan, &mut fake);

        assert_eq!(report.errors.len(), 1);
        assert!(
            !fake.methods().contains(&"pane.move"),
            "{:?}",
            fake.methods()
        );
        assert!(
            !fake.methods().contains(&"tab.create"),
            "{:?}",
            fake.methods()
        );
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn reorder_moves_only_what_is_out_of_place() {
        assert_eq!(reorder(ids(&["a", "b", "c"]), &ids(&["a", "b", "c"])), []);
        assert_eq!(
            reorder(ids(&["a", "b", "c", "x"]), &ids(&["c", "a", "gone", "b"])),
            [("c".to_owned(), 0)]
        );
        assert_eq!(
            reorder(ids(&["a", "b", "c"]), &ids(&["b", "c", "a"])),
            [("b".to_owned(), 0), ("c".to_owned(), 1)]
        );
    }

    #[test]
    fn the_origin_pane_is_focused_where_its_tab_went() {
        let plan = Plan {
            workspaces: vec![PlannedWorkspace {
                id: Some("w2".into()),
                label: None,
                tabs: vec![existing("w1:t1", "w1", None)],
            }],
            names: [("w1:t1", "server"), ("w2", "web")]
                .into_iter()
                .map(|(id, name)| (id.into(), name.into()))
                .collect(),
            focus: Some("w1:p1".into()),
            ..Plan::default()
        };
        let mut fake = Fake::default();
        let report = apply(&plan, &mut fake);

        assert_eq!(report.done, ["move \"server\" to \"web\""]);
        let (method, params) = fake.calls.last().unwrap();
        assert_eq!(method, "pane.focus");
        // The fake gives the moved pane a new id, as herdr does.
        let moved = fake
            .calls
            .iter()
            .position(|(m, _)| m == "pane.move")
            .unwrap();
        assert_eq!(params["pane_id"], format!("w2:p{}", moved + 1));
    }

    #[test]
    fn a_new_tab_is_focused_like_a_browser_tab() {
        let new = |label: &str| PlannedTab {
            source: TabSource::New,
            label: Some(label.into()),
        };
        let plan = Plan {
            workspaces: vec![PlannedWorkspace {
                id: Some("w1".into()),
                label: None,
                tabs: vec![existing("w2:t1", "w2", None), new("first"), new("last")],
            }],
            focus: Some("w2:p1".into()),
            ..Plan::default()
        };
        let mut fake = Fake::default();
        apply(&plan, &mut fake);

        let created: Vec<&Value> = fake
            .calls
            .iter()
            .filter(|(m, _)| m == "tab.create")
            .map(|(_, params)| params)
            .collect();
        assert_eq!(created.len(), 2);
        let last_created = fake
            .calls
            .iter()
            .rposition(|(m, _)| m == "tab.create")
            .unwrap();
        let (method, params) = fake.calls.last().unwrap();
        assert_eq!(method, "tab.focus");
        assert_eq!(params["tab_id"], format!("wN:t{}", last_created + 1));
        assert!(!fake.methods().contains(&"pane.focus"));
    }
}
