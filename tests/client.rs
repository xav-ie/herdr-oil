//! A real herdr client in a real terminal: tmux runs the client, keys go in
//! through tmux, and the screen is read back with `capture-pane`. The plugin is
//! opened with its keybinding, edited in vim inside the popup, and confirmed at
//! the prompt, the way a person would.
//!
//! Needs `tmux` and `vim` on PATH, besides the herdr in `HERDR_OIL_E2E_HERDR`.

mod common;

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::json;

use common::{Options, Server, herdr_binary, outline};

const KEYS: &str = r#"
[ui.toast]
delivery = "herdr"

[keys]
prefix = "ctrl+b"

[[keys.command]]
key = "prefix+o"
type = "plugin_action"
command = "xav-ie.herdr-oil.edit"
description = "Edit workspaces and tabs"
"#;

/// A herdr client attached to `server`, running in its own tmux server.
struct Client {
    socket: PathBuf,
}

impl Client {
    fn attach(server: &Server) -> Self {
        let socket = server.dir.join("tmux");
        let status = Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .args([
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-x",
                "110",
                "-y",
                "34",
            ])
            .arg(herdr_binary())
            .env_clear()
            .envs(server.env.iter().map(|(k, v)| (k, v)))
            .env("TERM", "xterm-256color")
            .stdin(Stdio::null())
            .status()
            .expect("running tmux; is it on PATH?");
        assert!(status.success());
        let client = Self { socket };
        client.wait_for("the herdr client to draw", |screen| {
            screen.contains("spaces")
        });
        client
    }

    fn tmux(&self, args: &[&str]) -> String {
        let output = Command::new("tmux")
            .arg("-S")
            .arg(&self.socket)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "tmux {args:?}: {output:?}");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Types `keys`, each a tmux key name or literal text.
    fn keys(&self, keys: &[&str]) {
        let mut args = vec!["send-keys"];
        args.extend_from_slice(keys);
        self.tmux(&args);
    }

    /// Escape on its own, then a pause: a terminal reads Escape followed at
    /// once by more keys as an Alt chord, and no person types that fast.
    fn escape(&self) {
        self.keys(&["Escape"]);
        std::thread::sleep(Duration::from_millis(300));
    }

    fn screen(&self) -> String {
        self.tmux(&["capture-pane", "-p"])
    }

    fn wait_for(&self, what: &str, check: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let screen = self.screen();
            if check(&screen) {
                return screen;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; screen:\n{screen}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn open_popup(&self) -> String {
        self.keys(&["C-b", "o"]);
        self.wait_for("the popup with vim in it", |screen| {
            screen.contains("┌Oil") && screen.contains("- /1 one")
        })
    }

    fn wait_for_popup_to_close(&self) {
        self.wait_for("the popup to close", |screen| !screen.contains("┌Oil"));
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .arg("-S")
            .arg(&self.socket)
            .arg("kill-server")
            .status();
    }
}

fn session() -> Server {
    let mut server = Server::start_with(&Options {
        env: vec![("EDITOR", "vim -u NONE -i NONE -N".to_owned())],
        plugin: true,
        config: KEYS,
    });
    server.workspace("alpha", &["one", "two", "doomed"]);
    server.workspace("beta", &["b"]);
    server
}

#[test]
#[ignore = "needs herdr, tmux and vim"]
fn move_and_close_in_vim_inside_the_popup() {
    let mut server = session();
    let client = Client::attach(&server);

    let screen = client.open_popup();
    // The outline is drawn inside the popup's border.
    assert!(
        screen
            .lines()
            .any(|line| line.contains("│/2 beta:") || line.contains("│  - /4 b")),
        "{screen}"
    );

    // Cut `one` (the tab the popup was opened from), paste it under beta's
    // tab, delete `doomed`, save and quit.
    client.keys(&["/one$", "Enter", "d", "d", "/ b$", "Enter", "p"]);
    client.keys(&[":g/doomed/d", "Enter", ":wq", "Enter"]);

    let screen = client.wait_for("the close prompt", |s| s.contains("Closing this tab"));
    assert!(screen.contains("✗ \"doomed\""), "{screen}");
    client.keys(&["y"]);

    // Moving `one` takes the popup's own pane away, so herdr ends the popup
    // partway through. The rest of the plan still has to happen, and the
    // result arrives as a toast instead, saying what was done.
    client.wait_for("the result toast", |s| {
        s.contains("Moved \"one\"; closed \"doomed\"")
    });
    client.wait_for_popup_to_close();

    assert_eq!(
        server.outline(),
        outline(&[("alpha", &["two"]), ("beta", &["b", "one"])])
    );
    // And focus went with `one`, the tab the popup was opened from.
    let snapshot = server.call("session.snapshot", json!({}));
    let focused = &snapshot["snapshot"]["focused_tab_id"];
    let one = &server.snapshot().workspaces[1].tabs[1];
    assert_eq!(focused.as_str(), Some(one.id.as_str()), "{snapshot}");
}

#[test]
#[ignore = "needs herdr, tmux and vim"]
fn quitting_vim_without_saving_changes_nothing() {
    let mut server = session();
    let client = Client::attach(&server);

    client.open_popup();
    client.keys(&[":g/doomed/d", "Enter", ":q!", "Enter"]);
    client.wait_for_popup_to_close();

    assert_eq!(
        server.outline(),
        outline(&[("alpha", &["one", "two", "doomed"]), ("beta", &["b"])])
    );
}

#[test]
#[ignore = "needs herdr, tmux and vim"]
fn a_mistake_reopens_vim_with_a_comment_above_it() {
    let mut server = session();
    let client = Client::attach(&server);

    client.open_popup();
    // A tab above every workspace, and a rename further down.
    client.keys(&["g", "g", "O", "- orphan"]);
    client.escape();
    client.keys(&["/two$", "Enter", "A", " too"]);
    client.escape();
    client.keys(&[":wq", "Enter"]);

    // Vim again, the comment above the line and the rename kept.
    let screen = client.wait_for("vim to reopen with the error", |s| {
        s.contains("# error: ") && s.contains("- /2 two too")
    });
    assert!(screen.contains("- orphan"), "{screen}");

    // Delete the comment and the orphan, save.
    client.keys(&["g", "g", "d", "j", ":wq", "Enter"]);
    client.wait_for_popup_to_close();
    // The popup outlived this edit, and the toast still says what was done.
    client.wait_for("the result toast", |s| {
        s.contains("Renamed tab \"two\" to \"two too\"")
    });

    assert_eq!(
        server.outline(),
        outline(&[("alpha", &["one", "two too", "doomed"]), ("beta", &["b"])])
    );
}
