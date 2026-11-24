//! The `herdr-oil` binary itself: the edit loop, its prompts, and how it
//! fails, driven with a scripted `$EDITOR` and answers on stdin.

mod common;

use std::io::Write;
use std::process::{Command, Output, Stdio};

use common::{Server, editor_script, outline};

/// Runs `herdr-oil edit` against `server` with `editor` and `answers` typed
/// at its prompts.
fn edit(server: &Server, editor: &str, answers: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_herdr-oil"))
        .arg("edit")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("TMPDIR", &server.dir)
        .env("HERDR_SOCKET_PATH", &server.socket)
        .env("EDITOR", editor)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(answers.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// alpha: one, two, doomed. The tests' scripts rename `one` and delete
/// `doomed`.
fn session() -> Server {
    let mut server = Server::start();
    server.workspace("alpha", &["one", "two", "doomed"]);
    server
}

const RENAME_AND_DELETE: &str = r"sed -i -e 's|^\(  - /1\) one$|\1 uno|' -e '/ doomed$/d' $1";

#[test]
#[ignore = "needs a herdr binary in HERDR_OIL_E2E_HERDR"]
fn closing_needs_a_yes_and_no_still_applies_the_rest() {
    for (answers, closes) in [
        ("y\n", true),
        ("yes\n", true),
        ("n\n", false),
        ("\n", false),
        ("", false),
    ] {
        let mut server = session();
        let editor = editor_script(&server.dir, "ed", RENAME_AND_DELETE);
        let output = edit(&server, &editor, answers);
        assert!(output.status.success(), "{output:?}");
        assert!(stdout(&output).contains("✗ \"doomed\""), "{output:?}");

        // The rename happens either way; only the close needs the yes.
        let expected: &[&str] = if closes {
            &["uno", "two"]
        } else {
            &["uno", "two", "doomed"]
        };
        assert_eq!(
            server.outline(),
            outline(&[("alpha", expected)]),
            "answers {answers:?}"
        );
    }
}

#[test]
#[ignore = "needs a herdr binary in HERDR_OIL_E2E_HERDR"]
fn edit_at_the_close_prompt_reopens_the_editor_with_the_edits_kept() {
    let mut server = session();
    // First run renames and deletes; second run sees the rename still there
    // and puts the deleted line back.
    let body = format!(
        "if [ -e {dir}/ran ]; then grep -q ' uno$' $1; echo '  - /3 doomed' >> $1; else touch {dir}/ran; {RENAME_AND_DELETE}; fi",
        dir = server.dir.display()
    );
    let editor = editor_script(&server.dir, "ed", &body);
    let output = edit(&server, &editor, "e\n");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        server.outline(),
        outline(&[("alpha", &["uno", "two", "doomed"])])
    );
}

#[test]
#[ignore = "needs a herdr binary in HERDR_OIL_E2E_HERDR"]
fn a_mistake_reopens_the_editor_with_it_commented() {
    let mut server = session();
    // First run puts a tab above every workspace and renames `one`; second
    // run finds the comment above it and the rename still there, and moves
    // the tab under the workspace.
    let body = format!(
        "if [ -e {dir}/ran ]; then grep -q ' uno$' $1; grep -q '^# error: ' $1; sed -i -e '/^- orphan$/d' -e '/^# error: /d' $1; echo '  - orphan' >> $1; else touch {dir}/ran; sed -i -e '1i - orphan' -e 's|^\\(  - /1\\) one$|\\1 uno|' $1; fi",
        dir = server.dir.display()
    );
    let editor = editor_script(&server.dir, "ed", &body);
    let output = edit(&server, &editor, "");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        server.outline(),
        outline(&[("alpha", &["uno", "two", "doomed", "orphan"])])
    );

    // Quitting without saving on the second round changes nothing.
    let mut server = session();
    let body = format!(
        "if [ ! -e {dir}/ran ]; then touch {dir}/ran; sed -i '1i - orphan' $1; fi",
        dir = server.dir.display()
    );
    let editor = editor_script(&server.dir, "ed", &body);
    let output = edit(&server, &editor, "");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        server.outline(),
        outline(&[("alpha", &["one", "two", "doomed"])])
    );
}

#[test]
#[ignore = "needs a herdr binary in HERDR_OIL_E2E_HERDR"]
fn quitting_without_saving_and_emptying_the_file_change_nothing() {
    for (body, says) in [
        // `:cq` in vim: exit non-zero.
        (
            format!("{RENAME_AND_DELETE}; exit 1"),
            "Editor exited with an error",
        ),
        // Quit without saving.
        ("true".to_owned(), ""),
        // Everything deleted.
        (": > $1".to_owned(), "Outline is empty"),
        // Only the help comment left.
        ("sed -i '/^#/!d' $1".to_owned(), "Outline is empty"),
    ] {
        let mut server = session();
        let editor = editor_script(&server.dir, "ed", &body);
        let output = edit(&server, &editor, "y\n");
        assert!(output.status.success(), "{output:?}");
        assert!(stdout(&output).contains(says), "{body}: {output:?}");
        assert_eq!(
            server.outline(),
            outline(&[("alpha", &["one", "two", "doomed"])]),
            "{body}"
        );
    }
}

#[test]
#[ignore = "needs a herdr binary in HERDR_OIL_E2E_HERDR"]
fn editors_with_arguments_work_and_the_file_is_private_and_removed() {
    let server = session();
    let seen = server.dir.join("seen");
    // `$EDITOR` with its own arguments, like `code --wait`.
    let editor = editor_script(
        &server.dir,
        "ed",
        &format!(
            "[ \"$1\" = --wait ]; shift; stat -c %a \"$1\" > {seen}; echo \"$1\" >> {seen}",
            seen = seen.display()
        ),
    );
    let output = edit(&server, &format!("{editor} --wait"), "");
    assert!(output.status.success(), "{output:?}");

    let seen = std::fs::read_to_string(seen).unwrap();
    let mut lines = seen.lines();
    assert_eq!(lines.next(), Some("600"));
    let path = lines.next().unwrap();
    assert_eq!(
        std::path::Path::new(path).extension(),
        Some(std::ffi::OsStr::new("yaml")),
        "{path}"
    );
    assert!(
        !std::path::Path::new(path).exists(),
        "{path} was left behind"
    );
}

#[test]
fn usage_and_missing_environment_fail_clearly() {
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_herdr-oil"))
            .args(args)
            .env_clear()
            .output()
            .unwrap()
    };

    let output = run(&[]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("usage: herdr-oil"));

    assert!(run(&["--help"]).status.success());

    let output = run(&["edit"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("HERDR_SOCKET_PATH"));

    let output = run(&["open"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("HERDR_PLUGIN_ID"));
}
