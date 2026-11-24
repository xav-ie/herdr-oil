//! A real, isolated, headless herdr server per test, and helpers to build
//! sessions in it and read them back.
//!
//! The end-to-end tests are ignored by default because they need a herdr
//! binary; run them with `HERDR_OIL_E2E_HERDR=$(command -v herdr) cargo test --
//! --include-ignored`, or inside `nix develop ./dev`, which sets it.

#![allow(dead_code)] // each test binary uses a different part

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use herdr_oil::apply::{Report, apply};
use herdr_oil::doc::{parse, render};
use herdr_oil::model::Snapshot;
use herdr_oil::plan::plan;
use herdr_oil::rpc::{Client, Herdr};

pub const PLUGIN_ID: &str = "xav-ie.herdr-oil";

pub struct Server {
    child: Child,
    pub dir: PathBuf,
    pub socket: PathBuf,
    pub client: Client,
    /// The environment every herdr process for this server runs with.
    pub env: Vec<(String, String)>,
}

#[derive(Default)]
pub struct Options<'a> {
    /// Extra environment for the server, which its plugin commands inherit.
    pub env: Vec<(&'a str, String)>,
    /// Link this crate as a plugin first, laid out the way the nix package is.
    pub plugin: bool,
    /// Appended to the server's config.toml.
    pub config: &'a str,
}

impl Server {
    pub fn start() -> Self {
        Self::start_with(&Options::default())
    }

    pub fn start_with(options: &Options) -> Self {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let herdr = herdr_binary();
        // Short: a Unix socket path must fit in about 100 bytes.
        let dir = std::env::temp_dir().join(format!(
            "ho-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        ));
        let config = dir.join("c");
        std::fs::create_dir_all(config.join("herdr")).unwrap();
        std::fs::write(
            config.join("herdr/config.toml"),
            format!(
                "onboarding = false\n[terminal]\ndefault_shell = \"sh\"\n{}",
                options.config
            ),
        )
        .unwrap();

        let mut env: Vec<(String, String)> = vec![
            ("PATH".into(), std::env::var("PATH").unwrap_or_default()),
            ("HOME".into(), dir.display().to_string()),
            ("XDG_CONFIG_HOME".into(), config.display().to_string()),
            ("SHELL".into(), "sh".into()),
        ];
        env.extend(
            options
                .env
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone())),
        );
        let command = |args: &[&str]| {
            let mut command = Command::new(&herdr);
            command
                .args(args)
                .env_clear()
                .envs(env.iter().map(|(k, v)| (k, v)))
                .stdin(Stdio::null());
            command
        };

        if options.plugin {
            let root = plugin_root(&dir.join("plugin"));
            let output = command(&["plugin", "link", root.to_str().unwrap()])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "plugin link: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let child = command(&["server"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("starting herdr server");

        let socket = config.join("herdr/herdr.sock");
        let mut client = Client::new(&socket);
        let deadline = Instant::now() + Duration::from_secs(20);
        while client.call("ping", json!({})).is_err() {
            assert!(Instant::now() < deadline, "herdr server never answered");
            std::thread::sleep(Duration::from_millis(50));
        }
        Self {
            child,
            dir,
            socket,
            client,
            env,
        }
    }

    pub fn call(&mut self, method: &str, params: Value) -> Value {
        self.client
            .call(method, params)
            .unwrap_or_else(|e| panic!("{e:#}"))
    }

    pub fn snapshot(&mut self) -> Snapshot {
        Snapshot::fetch(&mut self.client).unwrap()
    }

    /// Creates a workspace whose first tab has `tabs[0]`, then the other tabs.
    /// Returns the workspace id and tab ids.
    pub fn workspace(&mut self, label: &str, tabs: &[&str]) -> (String, Vec<String>) {
        let created = self.call("workspace.create", json!({ "label": label }));
        let id = id_of(&created["workspace"]["workspace_id"]);
        let first = id_of(&created["tab"]["tab_id"]);
        self.call("tab.rename", json!({ "tab_id": first, "label": tabs[0] }));
        let mut ids = vec![first];
        for label in &tabs[1..] {
            let tab = self.call("tab.create", json!({ "workspace_id": id, "label": label }));
            ids.push(id_of(&tab["tab"]["tab_id"]));
        }
        (id, ids)
    }

    /// The tab's only pane, before any split.
    pub fn root_pane(&mut self, tab_id: &str) -> String {
        let layout = self.call("layout.export", json!({ "tab_id": tab_id }));
        id_of(&layout["layout"]["root"]["pane_id"])
    }

    pub fn split(&mut self, pane_id: &str, direction: &str, ratio: f64) -> String {
        let split = self.call(
            "pane.split",
            json!({ "target_pane_id": pane_id, "direction": direction, "ratio": ratio }),
        );
        id_of(&split["pane"]["pane_id"])
    }

    /// Plans and applies `text`, written with herdr ids, as if it came back
    /// from the editor with the numbers of `before`'s outline, and expects no
    /// errors.
    pub fn edit(&mut self, before: &Snapshot, text: &str) -> (Report, Vec<String>) {
        let text = &numbered(before, text);
        let doc = parse(text).unwrap_or_else(|e| panic!("{e:?}\n{text}"));
        let now = self.snapshot();
        let plan = plan(before, &doc, &now);
        let report = apply(&plan, &mut self.client);
        assert!(report.errors.is_empty(), "{:#?}\n{text}", report.errors);
        (report, plan.warnings)
    }

    /// `(workspace label, [tab labels])` in sidebar order.
    pub fn outline(&mut self) -> Vec<(String, Vec<String>)> {
        self.snapshot()
            .workspaces
            .into_iter()
            .map(|w| (w.label, w.tabs.into_iter().map(|t| t.label).collect()))
            .collect()
    }

    /// The tab's layout with pane ids, cwds and float noise dropped, so
    /// layouts in different workspaces compare equal.
    pub fn shape(&mut self, tab_id: &str) -> Value {
        fn strip(node: &mut Value) {
            if let Some(map) = node.as_object_mut() {
                for key in ["pane_id", "cwd", "env", "command", "label"] {
                    map.remove(key);
                }
                if let Some(ratio) = map.get_mut("ratio") {
                    *ratio = json!((ratio.as_f64().unwrap() * 100.0).round() / 100.0);
                }
                for child in ["first", "second"] {
                    if let Some(child) = map.get_mut(child) {
                        strip(child);
                    }
                }
            }
        }
        let mut root =
            self.call("layout.export", json!({ "tab_id": tab_id }))["layout"]["root"].take();
        strip(&mut root);
        root
    }

    pub fn wait_for_output(&mut self, pane_id: &str, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let read = self.call(
                "pane.read",
                json!({ "pane_id": pane_id, "source": "recent" }),
            );
            if read.to_string().contains(needle) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{needle:?} never showed in {pane_id}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Polls until `check` holds, for changes made by another process.
    pub fn wait_until(&mut self, what: &str, mut check: impl FnMut(&mut Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !check(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn herdr_binary() -> String {
    std::env::var("HERDR_OIL_E2E_HERDR")
        .expect("set HERDR_OIL_E2E_HERDR to a herdr binary to run the e2e tests")
}

/// A plugin root like the nix package's: the manifest without `[[build]]`,
/// and this test build's binary where the manifest expects the release one.
fn plugin_root(root: &Path) -> PathBuf {
    let manifest =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/herdr-plugin.toml")).unwrap();
    let mut stripped = String::new();
    let mut skipping = false;
    for line in manifest.lines() {
        if line.starts_with("[[") {
            skipping = line == "[[build]]";
        }
        if !skipping {
            stripped.push_str(line);
            stripped.push('\n');
        }
    }
    std::fs::create_dir_all(root.join("target/release")).unwrap();
    std::fs::write(root.join("herdr-plugin.toml"), stripped).unwrap();
    std::os::unix::fs::symlink(
        env!("CARGO_BIN_EXE_herdr-oil"),
        root.join("target/release/herdr-oil"),
    )
    .unwrap();
    root.to_owned()
}

/// The numbers `before`'s outline gives its workspaces and its tabs, by id.
fn numbers(before: &Snapshot) -> (Vec<String>, Vec<String>) {
    let workspaces = before.workspaces.iter().map(|w| w.id.clone()).collect();
    let tabs = before
        .workspaces
        .iter()
        .flat_map(|w| &w.tabs)
        .map(|t| t.id.clone())
        .collect();
    (workspaces, tabs)
}

/// `text` with each `/id` (`/w1`, `/w1:t2`) replaced by the number
/// `before`'s outline gives it, so tests can write outlines by id.
pub fn numbered(before: &Snapshot, text: &str) -> String {
    let (workspaces, tabs) = numbers(before);
    let mut out = String::new();
    for line in text.lines() {
        let mut line = line.to_owned();
        if let Some(slash) = line.find('/') {
            let rest = &line[slash + 1..];
            let end = rest.find([' ', '"']).unwrap_or(rest.len());
            let token = rest[..end].trim_end_matches(':');
            let number = tabs
                .iter()
                .chain(&workspaces)
                .position(|id| id == token)
                .map(|i| {
                    if i < tabs.len() {
                        i + 1
                    } else {
                        i - tabs.len() + 1
                    }
                });
            if let Some(number) = number {
                line.replace_range(slash + 1..slash + 1 + token.len(), &number.to_string());
            }
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// `before`'s outline as the editor gets it, but with herdr ids in place of
/// numbers, for a test to edit by id and hand to [`Server::edit`].
pub fn render_ids(before: &Snapshot) -> String {
    let (workspaces, tabs) = numbers(before);
    let mut out = String::new();
    for line in render(before).lines() {
        let mut line = line.to_owned();
        let trimmed = line.trim_start();
        let ids = if trimmed.starts_with("- ") {
            &tabs
        } else {
            &workspaces
        };
        if !trimmed.starts_with('#')
            && let Some(slash) = line.find('/')
        {
            let rest = &line[slash + 1..];
            let end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            if let Ok(n) = rest[..end].parse::<usize>() {
                line.replace_range(slash + 1..slash + 1 + end, &ids[n - 1]);
            }
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

pub fn id_of(value: &Value) -> String {
    value
        .as_str()
        .unwrap_or_else(|| panic!("expected an id, got {value}"))
        .to_owned()
}

pub fn outline(expected: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
    expected
        .iter()
        .map(|(w, tabs)| {
            (
                (*w).to_owned(),
                tabs.iter().map(|t| (*t).to_owned()).collect(),
            )
        })
        .collect()
}

/// An `$EDITOR` value that runs `body` as a shell script with the file in
/// `$1`. Run through `sh` rather than a shebang, which the nix sandbox has
/// no `/usr/bin/env` for.
pub fn editor_script(dir: &Path, name: &str, body: &str) -> String {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, format!("set -eu\n{body}\n")).unwrap();
    format!("sh {}", path.display())
}
