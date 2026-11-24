use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde_json::json;

use herdr_oil::apply::{Report, apply};
use herdr_oil::doc;
use herdr_oil::model::Snapshot;
use herdr_oil::plan::{Plan, plan};
use herdr_oil::rpc::{Client, Herdr};

const USAGE: &str = "\
usage: herdr-oil <command>

  open    open the editor in a herdr popup (what the plugin action runs)
  edit    edit the outline in this terminal and apply it

  apply   internal: the plugin action the popup hands a confirmed plan to";

fn main() -> ExitCode {
    let result = match std::env::args().nth(1).as_deref() {
        Some("open") => open(),
        Some("edit") => edit(),
        Some("apply") => apply_action(),
        Some("-h" | "--help") => {
            println!("{USAGE}");
            Ok(())
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("herdr-oil: {error:#}");
            // A popup closes with its process, so hold the message on screen.
            if std::env::var_os("HERDR_PLUGIN_ENTRYPOINT_ID").is_some() {
                pause();
            }
            ExitCode::FAILURE
        }
    }
}

/// The plugin action: actions run without a terminal, so open the `editor`
/// entrypoint as a popup.
fn open() -> Result<()> {
    let plugin_id = std::env::var("HERDR_PLUGIN_ID")
        .context("HERDR_PLUGIN_ID is not set; run this from the herdr plugin action")?;
    Client::from_env()?.call(
        "plugin.pane.open",
        json!({ "plugin_id": plugin_id, "entrypoint": "editor", "focus": true }),
    )?;
    Ok(())
}

fn edit() -> Result<()> {
    let mut herdr = Client::from_env()?;
    let before = Snapshot::fetch(&mut herdr)?;
    // What the editor was last given: closing it with the file still the same
    // means quitting without saving.
    let mut given = doc::render(&before);
    let file = TempFile::create("yaml", &given)?;

    loop {
        if !run_editor(&file.path)? {
            println!("Editor exited with an error; nothing changed.");
            return Ok(());
        }
        let text = fs::read_to_string(&file.path)?;
        if text == given {
            return Ok(());
        }

        let doc = match doc::parse(&text) {
            Ok(doc) if doc.workspaces.is_empty() => {
                println!("Outline is empty; nothing changed.");
                return Ok(());
            }
            Ok(doc) => doc,
            // Back to the editor, each mistake a comment above its line.
            Err(errors) => {
                given = doc::annotate(&text, &errors);
                fs::write(&file.path, &given)?;
                continue;
            }
        };
        let mut plan = plan(&before, &doc, &Snapshot::fetch(&mut herdr)?);
        plan.focus = origin_pane(&mut herdr);

        if plan.closes_anything() {
            print_closes(&plan);
            match key(&["y", "e", "n"], "n")? {
                "y" => {}
                "e" => continue,
                // Everything but the closes still happens; what was to close
                // stays where it is.
                _ => {
                    plan.close_tabs.clear();
                    plan.close_workspaces.clear();
                }
            }
        }

        let report = apply_plan(&plan, &mut herdr, &file.path)?;
        print_report(&plan, &report);
        if !report.errors.is_empty() || !plan.warnings.is_empty() {
            pause();
        }
        return Ok(());
    }
}

/// How long the `apply` action waits for the popup to collect its report
/// before deciding the popup is gone.
const COLLECT_GRACE: Duration = Duration::from_millis(1500);

/// Applies `plan`, through the `apply` plugin action when running as the
/// popup.
///
/// A popup is tied to the pane that was focused when it opened, and herdr
/// hangs up the popup's whole terminal session the moment that pane moves or
/// closes: exactly what moving or closing the current tab does, partway
/// through the plan. An action runs outside any terminal, so the plan is handed
/// to one, and it finishes the plan either way. This process waits for the
/// report and shows it, if it is still around to.
fn apply_plan(plan: &Plan, herdr: &mut Client, outline: &Path) -> Result<Report> {
    let (Some(plugin_id), Some(state)) = (
        std::env::var_os("HERDR_PLUGIN_ID"),
        std::env::var_os("HERDR_PLUGIN_STATE_DIR"),
    ) else {
        return Ok(apply(plan, herdr));
    };
    let state = PathBuf::from(state);
    let pending = state.join("pending");
    fs::create_dir_all(&pending)?;

    let id = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let envelope = json!({ "plan": plan, "outline": outline });
    write_atomically(
        &pending.join(format!("{id}.json")),
        &serde_json::to_vec(&envelope)?,
    )?;
    herdr.call(
        "plugin.action.invoke",
        json!({ "action_id": format!("{}.apply", plugin_id.to_string_lossy()) }),
    )?;

    let report_path = state.join(format!("report-{id}.json"));
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        if let Ok(report) = fs::read(&report_path) {
            let _ = fs::remove_file(&report_path);
            return Ok(serde_json::from_slice(&report)?);
        }
        if Instant::now() > deadline {
            bail!("the apply action never reported back; see `herdr plugin log list`");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The `apply` action: applies every pending plan, says what was done with a
/// toast, and leaves the report for its popup. When a popup doesn't collect
/// its report (herdr ended it mid plan), it removes the outline file the popup
/// would have.
fn apply_action() -> Result<()> {
    let state = PathBuf::from(
        std::env::var_os("HERDR_PLUGIN_STATE_DIR")
            .context("HERDR_PLUGIN_STATE_DIR is not set; the popup runs this action")?,
    );
    let mut herdr = Client::from_env()?;
    let mut pending: Vec<PathBuf> = fs::read_dir(state.join("pending"))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .collect();
    pending.sort();

    for path in pending {
        // Claimed by renaming, so two invocations never apply the same plan.
        let claimed = path.with_extension("claimed");
        if fs::rename(&path, &claimed).is_err() {
            continue;
        }
        let envelope: serde_json::Value = serde_json::from_slice(&fs::read(&claimed)?)?;
        let _ = fs::remove_file(&claimed);
        let plan: Plan = serde_json::from_value(envelope["plan"].clone())?;
        let report = apply(&plan, &mut herdr);
        herdr.call(
            "notification.show",
            json!({ "title": "herdr-oil", "body": summary(&plan, &report) }),
        )?;

        let id = path.file_stem().unwrap_or_default().to_string_lossy();
        let report_path = state.join(format!("report-{id}.json"));
        write_atomically(&report_path, &serde_json::to_vec(&report)?)?;

        let deadline = Instant::now() + COLLECT_GRACE;
        while report_path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        if fs::remove_file(&report_path).is_ok()
            && let Some(outline) = envelope["outline"].as_str()
        {
            let _ = fs::remove_file(outline);
        }
    }
    Ok(())
}

fn write_atomically(path: &Path, contents: &[u8]) -> Result<()> {
    let partial = path.with_extension("partial");
    fs::write(&partial, contents)?;
    fs::rename(&partial, path)?;
    Ok(())
}

/// One line for a toast, which herdr cuts off at the screen edge: a single
/// change in full, several by what happened to which names, or just counts when
/// that is still too long; and the first problem if any.
fn summary(plan: &Plan, report: &Report) -> String {
    const FITS: usize = 72;
    let done: Vec<String> = report.done.iter().map(|step| shorten_names(step)).collect();
    let mut line = match done.as_slice() {
        [] => "nothing changed".to_owned(),
        [step] => past(step),
        steps => {
            // `move "a" to position 2` is a reorder, which other changes
            // mostly cause, so it is only told when nothing else happened.
            // Anything else is its verb and the first quoted name.
            let reorder = |step: &&String| step.contains(" to position ");
            let mut told: Vec<&String> = steps.iter().filter(|s| !reorder(s)).collect();
            if told.is_empty() {
                told = steps.iter().collect();
            }
            let mut groups: Vec<(String, Vec<&str>)> = Vec::new();
            for step in told {
                let verb = if reorder(&step) {
                    "reordered".to_owned()
                } else {
                    past(step.split(' ').next().unwrap_or_default())
                };
                let name = step.split('"').nth(1).unwrap_or_default();
                match groups.iter_mut().find(|(v, _)| *v == verb) {
                    Some((_, names)) => names.push(name),
                    None => groups.push((verb, vec![name])),
                }
            }
            let named = groups
                .iter()
                .map(|(verb, names)| {
                    let names: Vec<String> = names.iter().map(|n| format!("\"{n}\"")).collect();
                    format!("{verb} {}", names.join(", "))
                })
                .collect::<Vec<_>>()
                .join("; ");
            if named.chars().count() <= FITS {
                named
            } else {
                let counts = groups
                    .iter()
                    .map(|(verb, names)| format!("{} {verb}", names.len()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{} changes: {counts}", steps.len())
            }
        }
    };
    if let Some(first) = report.errors.first() {
        let _ = write!(line, "; {} failed: {first}", report.errors.len());
    } else if let Some(first) = plan.warnings.first() {
        let _ = write!(line, "; {first}");
    }
    let mut chars = line.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// `step` with each quoted name cut to a length a toast has room for.
fn shorten_names(step: &str) -> String {
    const LONGEST: usize = 24;
    step.split('"')
        .enumerate()
        .map(|(i, part)| {
            // Odd parts are inside quotes.
            if i % 2 == 1 && part.chars().count() > LONGEST {
                let cut: String = part.chars().take(LONGEST - 1).collect();
                format!("{}…", cut.trim_end())
            } else {
                part.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\"")
}

/// A step as done: `move "a" to "b"` becomes `moved "a" to "b"`.
fn past(step: &str) -> String {
    let (verb, rest) = step.split_once(' ').unwrap_or((step, ""));
    let verb = match verb {
        "close" | "create" | "move" | "rename" => format!("{verb}d"),
        _ => verb.to_owned(),
    };
    if rest.is_empty() {
        verb
    } else {
        format!("{verb} {rest}")
    }
}

/// The pane the editor was opened from: the plugin context's when running as
/// the popup, the focused one otherwise.
fn origin_pane(herdr: &mut Client) -> Option<String> {
    let from_context = std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
        .ok()
        .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
        .and_then(|context| context["focused_pane_id"].as_str().map(str::to_owned));
    from_context.or_else(|| {
        let result = herdr.call("session.snapshot", serde_json::json!({})).ok()?;
        result["snapshot"]["focused_pane_id"]
            .as_str()
            .map(str::to_owned)
    })
}

/// The confirmation before anything closes, on a fresh screen: the editor
/// just left it, and a half-redrawn popup would show the two mixed.
fn print_closes(plan: &Plan) {
    let style = Style::new();
    if style.on {
        // Cleared, with the cursor hidden, and left a moment first: herdr
        // redraws the popup in steps once the editor gives the screen back,
        // and drawing straight away shows the prompt over the editor's
        // leftovers.
        print!("\x1b[?25l\x1b[0m\x1b[H\x1b[2J");
        let _ = io::stdout().flush();
        std::thread::sleep(Duration::from_millis(100));
        print!("\x1b[H\x1b[2J");
    }
    let (heading, it) = closing(plan);
    println!(
        "{}Closing {heading}{} stops {}:\n",
        style.bold,
        style.reset,
        if it == "it" {
            "its processes"
        } else {
            "their processes"
        }
    );
    let details = |tab: &herdr_oil::model::Tab| {
        let mut line = String::new();
        if tab.pane_count > 1 {
            let _ = write!(line, " · {} panes", tab.pane_count);
        }
        if !tab.agents.is_empty() {
            let _ = write!(line, " · running {}", tab.agents.join(", "));
        }
        line
    };
    for workspace in &plan.close_workspaces {
        println!(
            "  {red}✗{r} \"{}\"  {dim}workspace{r}",
            workspace.label,
            red = style.red,
            dim = style.dim,
            r = style.reset
        );
        for tab in &workspace.tabs {
            println!(
                "      \"{}\"{dim}{}{r}",
                tab.label,
                details(tab),
                dim = style.dim,
                r = style.reset
            );
        }
    }
    for tab in &plan.close_tabs {
        let workspace = tab
            .id
            .split_once(':')
            .and_then(|(workspace, _)| plan.names.get(workspace));
        let place = workspace
            .map(|w| format!("  {}in{} \"{w}\"", style.dim, style.reset))
            .unwrap_or_default();
        println!(
            "  {red}✗{r} \"{}\"{place}{dim}{}{r}",
            tab.label,
            details(tab),
            red = style.red,
            dim = style.dim,
            r = style.reset
        );
    }
    print!(
        "\n  {b}y{r}  yes, close {it}\n  {b}e{r}  edit actions\n  {b}n{r}  no, don't close {it}\n\n  ",
        b = style.bold,
        r = style.reset
    );
    if style.on {
        print!("\x1b[?25h");
    }
    let _ = io::stdout().flush();
}

/// What closes, for the heading ("this tab", "these 2 workspaces and 3
/// tabs"), and whether to call it "it" or "them".
fn closing(plan: &Plan) -> (String, &'static str) {
    let workspaces = plan.close_workspaces.len();
    let tabs = plan.close_tabs.len()
        + plan
            .close_workspaces
            .iter()
            .map(|w| w.tabs.len())
            .sum::<usize>();
    let count = |n: usize, one: &str, many: &str| {
        if n == 1 {
            format!("this {one}")
        } else {
            format!("these {n} {many}")
        }
    };
    let heading = match (workspaces, tabs) {
        (0, tabs) => count(tabs, "tab", "tabs"),
        (workspaces, 0) => count(workspaces, "workspace", "workspaces"),
        (workspaces, 1) => format!("{} and 1 tab", count(workspaces, "workspace", "workspaces")),
        (workspaces, tabs) => {
            format!(
                "{} and {tabs} tabs",
                count(workspaces, "workspace", "workspaces")
            )
        }
    };
    let single = workspaces + plan.close_tabs.len() == 1;
    (heading, if single { "it" } else { "them" })
}

/// ANSI styling, only when printing to a terminal.
struct Style {
    on: bool,
    bold: &'static str,
    dim: &'static str,
    red: &'static str,
    reset: &'static str,
}

impl Style {
    fn new() -> Self {
        use std::io::IsTerminal as _;
        let on = io::stdout().is_terminal();
        let pick = |code: &'static str| if on { code } else { "" };
        Self {
            on,
            bold: pick("\x1b[1m"),
            dim: pick("\x1b[2m"),
            red: pick("\x1b[31m"),
            reset: pick("\x1b[0m"),
        }
    }
}

fn print_report(plan: &Plan, report: &Report) {
    for line in &report.done {
        println!("✓ {line}");
    }
    for warning in &plan.warnings {
        println!("! {warning}");
    }
    for error in &report.errors {
        println!("✗ {error}");
    }
}

/// Opens `path` in `$VISUAL` or `$EDITOR` through `sh`, so editors given with
/// arguments (`code --wait`) work. Returns whether the editor succeeded.
fn run_editor(path: &Path) -> Result<bool> {
    let editor = ["VISUAL", "EDITOR"]
        .iter()
        .find_map(|var| std::env::var(var).ok().filter(|v| !v.trim().is_empty()))
        .unwrap_or_else(|| "vi".to_owned());
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("herdr-oil")
        .arg(path)
        .status()
        .with_context(|| format!("starting {editor}"))?;
    Ok(status.success())
}

/// One answer from `choices`, by its first letter. In a terminal a single key
/// is enough; otherwise a line is read. Anything else, Enter or end of input
/// means `default`.
fn key<'a>(choices: &[&'a str], default: &'a str) -> Result<&'a str> {
    let answer = read_key()?.to_ascii_lowercase();
    println!();
    Ok(choices
        .iter()
        .find(|choice| choice.starts_with(answer))
        .unwrap_or(&default))
}

/// A key from a terminal, through `stty` so no raw terminal handling is needed
/// here, or the first letter of a line from anything else.
fn read_key() -> Result<char> {
    use std::io::{IsTerminal as _, Read as _};
    if !io::stdin().is_terminal() {
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;
        return Ok(line.trim().chars().next().unwrap_or('\n'));
    }
    let saved = Command::new("stty").arg("-g").output()?;
    let saved = String::from_utf8_lossy(&saved.stdout).trim().to_owned();
    Command::new("stty")
        .args(["-icanon", "-echo", "min", "1"])
        .status()?;
    let mut byte = [0];
    let read = io::stdin().read(&mut byte);
    Command::new("stty").arg(&saved).status()?;
    read?;
    Ok(char::from(byte[0]))
}

fn pause() {
    print!("Press any key to close.");
    let _ = io::stdout().flush();
    let _ = read_key();
}

/// A file private to the user, removed on drop.
struct TempFile {
    path: PathBuf,
}

impl TempFile {
    fn create(extension: &str, contents: &str) -> Result<Self> {
        let path =
            std::env::temp_dir().join(format!("herdr-oil-{}.{extension}", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| format!("creating {}", path.display()))?;
        file.write_all(contents.as_bytes())?;
        Ok(Self { path })
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(done: &[&str], errors: &[&str]) -> Report {
        Report {
            done: done.iter().map(|s| (*s).to_owned()).collect(),
            errors: errors.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn the_toast_says_what_was_done_in_one_line() {
        let plan = Plan::default();
        let toast = |done: &[&str], errors: &[&str]| summary(&plan, &report(done, errors));

        assert_eq!(
            toast(&["move \"server\" to \"web\""], &[]),
            "Moved \"server\" to \"web\""
        );
        assert_eq!(
            toast(
                &[
                    "create tab \"review PR 42\" in \"api\"",
                    "create workspace \"ops\"",
                    "close tab \"old experiment\"",
                    "move \"ops\" to position 2 in \"api\"",
                ],
                &[]
            ),
            "Created \"review PR 42\", \"ops\"; closed \"old experiment\""
        );
        let many: Vec<String> = (0..6)
            .map(|i| format!("move \"a long tab name {i}\" to \"web\""))
            .collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(
            toast(&many, &["close tab \"x\": gone"]),
            "6 changes: 6 moved; 1 failed: close tab \"x\": gone"
        );
        assert_eq!(
            toast(
                &[
                    "move \"a\" to position 2 in \"web\"",
                    "move workspace \"web\" to position 1"
                ],
                &[]
            ),
            "Reordered \"a\", \"web\""
        );
        assert_eq!(
            toast(
                &["move \"server\" to \"web\"", "move \"tests\" to \"web\""],
                &[]
            ),
            "Moved \"server\", \"tests\""
        );
        assert_eq!(
            toast(
                &["move \"a really very extremely long tab name\" to \"web\""],
                &[]
            ),
            "Moved \"a really very extremely…\" to \"web\""
        );
        assert_eq!(toast(&[], &[]), "Nothing changed");
    }

    #[test]
    fn the_close_prompt_counts_what_closes() {
        use herdr_oil::model::Tab;
        use herdr_oil::plan::ClosingWorkspace;
        let tab = |id: &str| Tab {
            id: id.into(),
            label: id.into(),
            pane_count: 1,
            agents: Vec::new(),
        };
        let mut plan = Plan {
            close_tabs: vec![tab("w1:t1")],
            ..Plan::default()
        };
        assert_eq!(closing(&plan), ("this tab".into(), "it"));

        plan.close_tabs.push(tab("w1:t2"));
        assert_eq!(closing(&plan), ("these 2 tabs".into(), "them"));

        plan.close_workspaces.push(ClosingWorkspace {
            id: "w2".into(),
            label: "old".into(),
            tabs: vec![tab("w2:t1")],
        });
        assert_eq!(closing(&plan), ("this workspace and 3 tabs".into(), "them"));

        plan.close_tabs.clear();
        assert_eq!(closing(&plan), ("this workspace and 1 tab".into(), "it"));
    }
}
