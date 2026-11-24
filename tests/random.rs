//! Randomized end-to-end test. Each case builds a random session (workspaces,
//! tabs, split layouts), then for a few rounds makes random edits to its
//! outline, applies them, and checks herdr matches the edited outline exactly:
//! workspace and tab order, labels, and every surviving tab's split layout.
//!
//! `HERDR_OIL_RANDOM_CASES` sets how many seeds run (default 40);
//! `HERDR_OIL_RANDOM_SEED` reruns one seed from a failure message.

mod common;

use std::collections::HashMap;
use std::fmt::Write as _;

use serde_json::Value;

use common::Server;

/// `SplitMix64`: tiny, seedable, and good enough to pick edits.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number in `0..n`.
    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % (n as u64)).unwrap()
    }

    fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len())]
    }
}

/// The outline as the test means it to be.
#[derive(Clone, Debug)]
struct Model {
    workspaces: Vec<ModelWorkspace>,
}

#[derive(Clone, Debug)]
struct ModelWorkspace {
    id: Option<String>,
    label: String,
    tabs: Vec<ModelTab>,
}

#[derive(Clone, Debug)]
struct ModelTab {
    id: Option<String>,
    label: String,
}

impl Model {
    fn from_server(server: &mut Server) -> Self {
        Self {
            workspaces: server
                .snapshot()
                .workspaces
                .into_iter()
                .map(|w| ModelWorkspace {
                    id: Some(w.id),
                    label: w.label,
                    tabs: w
                        .tabs
                        .into_iter()
                        .map(|t| ModelTab {
                            id: Some(t.id),
                            label: t.label,
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    fn render(&self) -> String {
        let mut out = String::new();
        for workspace in &self.workspaces {
            match &workspace.id {
                Some(id) => writeln!(out, "\n/{id} {}:", workspace.label),
                None => writeln!(out, "\n{}:", workspace.label),
            }
            .unwrap();
            for tab in &workspace.tabs {
                match &tab.id {
                    Some(id) => writeln!(out, "  - /{id} {}", tab.label),
                    None => writeln!(out, "  - {}", tab.label),
                }
                .unwrap();
            }
        }
        out
    }

    fn outline(&self) -> Vec<(String, Vec<String>)> {
        self.workspaces
            .iter()
            .map(|w| {
                (
                    w.label.clone(),
                    w.tabs.iter().map(|t| t.label.clone()).collect(),
                )
            })
            .collect()
    }

    fn tab_count(&self) -> usize {
        self.workspaces.iter().map(|w| w.tabs.len()).sum()
    }

    /// Removes a random tab, returning it.
    fn take_tab(&mut self, rng: &mut Rng) -> ModelTab {
        let candidates: Vec<usize> = (0..self.workspaces.len())
            .filter(|&w| !self.workspaces[w].tabs.is_empty())
            .collect();
        let w = rng.pick(&candidates);
        let tabs = &mut self.workspaces[w].tabs;
        let t = rng.below(tabs.len());
        tabs.remove(t)
    }

    fn insert_tab(&mut self, rng: &mut Rng, tab: ModelTab) {
        let w = rng.below(self.workspaces.len());
        let tabs = &mut self.workspaces[w].tabs;
        let at = rng.below(tabs.len() + 1);
        tabs.insert(at, tab);
    }
}

/// Makes `count` random edits, the way someone in an editor would.
fn edit(model: &mut Model, rng: &mut Rng, count: usize, names: &mut usize) -> Vec<String> {
    let mut log = Vec::new();
    let mut name = |prefix: &str| {
        *names += 1;
        format!("{prefix}{names}")
    };
    for _ in 0..count {
        match rng.below(9) {
            0 | 1 => {
                let tab = model.take_tab(rng);
                log.push(format!("move tab {:?}", tab.label));
                model.insert_tab(rng, tab);
            }
            2 => {
                let w = rng.below(model.workspaces.len());
                let workspace = model.workspaces.remove(w);
                let at = rng.below(model.workspaces.len() + 1);
                log.push(format!("move workspace {:?} to {at}", workspace.label));
                model.workspaces.insert(at, workspace);
            }
            3 => {
                let label = name("renamed tab ");
                let tab = model.take_tab(rng);
                log.push(format!("rename tab {:?} to {label:?}", tab.label));
                model.insert_tab(rng, ModelTab { label, ..tab });
            }
            4 => {
                let label = name("renamed space ");
                let w = rng.below(model.workspaces.len());
                log.push(format!(
                    "rename workspace {:?} to {label:?}",
                    model.workspaces[w].label
                ));
                model.workspaces[w].label = label;
            }
            5 => {
                let label = name("new tab ");
                log.push(format!("create tab {label:?}"));
                model.insert_tab(rng, ModelTab { id: None, label });
            }
            6 => {
                let label = name("new space ");
                let first = if model.tab_count() > 1 && rng.chance(2) {
                    model.take_tab(rng)
                } else {
                    ModelTab {
                        id: None,
                        label: name("new tab "),
                    }
                };
                log.push(format!("create workspace {label:?} from {:?}", first.label));
                let at = rng.below(model.workspaces.len() + 1);
                model.workspaces.insert(
                    at,
                    ModelWorkspace {
                        id: None,
                        label,
                        tabs: vec![first],
                    },
                );
            }
            7 if model.workspaces.len() > 1 => {
                // Every tab of one workspace leaves and a new one takes their
                // place, so the workspace must not close as the last one goes.
                let w = rng.below(model.workspaces.len());
                let leaving = std::mem::take(&mut model.workspaces[w].tabs);
                let label = name("new tab ");
                log.push(format!(
                    "empty workspace {:?} into others, leaving {label:?}",
                    model.workspaces[w].label
                ));
                model.workspaces[w].tabs.push(ModelTab { id: None, label });
                for tab in leaving {
                    let mut to = rng.below(model.workspaces.len() - 1);
                    if to >= w {
                        to += 1;
                    }
                    let tabs = &mut model.workspaces[to].tabs;
                    let at = rng.below(tabs.len() + 1);
                    tabs.insert(at, tab);
                }
            }
            _ => {
                if rng.chance(3) && model.workspaces.len() > 1 {
                    let w = rng.below(model.workspaces.len());
                    let gone = model.workspaces.remove(w);
                    log.push(format!("delete workspace {:?}", gone.label));
                } else if model.tab_count() > 1 {
                    let gone = model.take_tab(rng);
                    log.push(format!("delete tab {:?}", gone.label));
                }
            }
        }
        // A workspace left without tabs is a workspace line the user deleted.
        model.workspaces.retain(|w| !w.tabs.is_empty());
    }
    log
}

/// Splits random panes of `tab_id` a few times.
fn random_layout(server: &mut Server, rng: &mut Rng, tab_id: &str) {
    let mut panes = vec![server.root_pane(tab_id)];
    for _ in 0..rng.below(4) {
        let target = panes[rng.below(panes.len())].clone();
        let direction = rng.pick(&["right", "down"]);
        let ratio = rng.pick(&[0.3, 0.4, 0.5, 0.6, 0.7]);
        panes.push(server.split(&target, direction, ratio));
    }
}

fn run_case(seed: u64) {
    let mut rng = Rng(seed);
    let mut server = Server::start();
    let mut names = 0;

    for w in 0..2 + rng.below(3) {
        let labels: Vec<String> = (0..=rng.below(4)).map(|t| format!("tab {w}.{t}")).collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let (_, tabs) = server.workspace(&format!("space {w}"), &refs);
        for tab in tabs {
            if rng.chance(2) {
                random_layout(&mut server, &mut rng, &tab);
            }
        }
    }

    for round in 0..3 {
        let before = server.snapshot();
        let mut model = Model::from_server(&mut server);
        let shapes: HashMap<String, Value> = before
            .workspaces
            .iter()
            .flat_map(|w| &w.tabs)
            .map(|t| t.id.clone())
            .collect::<Vec<_>>()
            .into_iter()
            .map(|id| {
                let shape = server.shape(&id);
                (id, shape)
            })
            .collect();

        let count = 1 + rng.below(8);
        let log = edit(&mut model, &mut rng, count, &mut names);
        let text = model.render();
        let context = format!(
            "seed {seed} round {round}; rerun with HERDR_OIL_RANDOM_SEED={seed}\nedits: {log:#?}\noutline:\n{text}"
        );

        let (_, warnings) = server.edit(&before, &text);
        assert_eq!(warnings, Vec::<String>::new(), "{context}");
        assert_eq!(server.outline(), model.outline(), "{context}");

        // Every tab that existed before keeps its layout, wherever it went.
        let after = server.snapshot();
        for (workspace, model_workspace) in after.workspaces.iter().zip(&model.workspaces) {
            for (tab, model_tab) in workspace.tabs.iter().zip(&model_workspace.tabs) {
                if let Some(old) = &model_tab.id {
                    assert_eq!(
                        server.shape(&tab.id),
                        shapes[old],
                        "layout of {:?}\n{context}",
                        model_tab.label
                    );
                }
            }
        }
    }
}

#[test]
#[ignore = "needs a herdr binary in HERDR_OIL_E2E_HERDR"]
fn random_edits_apply_exactly() {
    if let Ok(seed) = std::env::var("HERDR_OIL_RANDOM_SEED") {
        run_case(seed.parse().expect("HERDR_OIL_RANDOM_SEED is a number"));
        return;
    }
    let cases: u64 = std::env::var("HERDR_OIL_RANDOM_CASES")
        .ok()
        .map_or(40, |n| {
            n.parse().expect("HERDR_OIL_RANDOM_CASES is a number")
        });
    for seed in 0..cases {
        run_case(seed);
    }
}
