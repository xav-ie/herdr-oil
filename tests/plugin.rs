//! The whole plugin path: herdr runs the manifest's action, which opens the
//! popup, which runs `$EDITOR` and applies the result. The plugin is linked
//! from a root laid out like the nix package's, `[[build]]` stripped.

mod common;

use serde_json::json;

use common::{Options, PLUGIN_ID, Server, editor_script, outline};

#[test]
#[ignore = "needs a herdr binary in HERDR_OIL_E2E_HERDR"]
fn the_action_opens_the_editor_in_a_popup_and_applies_the_outline() {
    let dir = std::env::temp_dir().join(format!("ho-plugin-{}", std::process::id()));
    let seen = dir.join("seen.yaml");
    let editor = editor_script(
        &dir,
        "ed",
        &format!(
            "cp $1 {seen}; sed -i -e 's|^\\(  - /1\\) one$|\\1 from the popup|' $1",
            seen = seen.display()
        ),
    );
    let mut server = Server::start_with(&Options {
        env: vec![("EDITOR", editor)],
        plugin: true,
        ..Options::default()
    });
    server.workspace("alpha", &["one", "two"]);

    let actions = server.call("plugin.action.list", json!({ "plugin_id": PLUGIN_ID }));
    assert!(actions.to_string().contains("\"edit\""), "{actions}");

    let invoked = server.call(
        "plugin.action.invoke",
        json!({ "action_id": format!("{PLUGIN_ID}.edit") }),
    );
    assert_eq!(invoked["type"], "plugin_action_invoked", "{invoked}");

    server.wait_until("the popup to apply the outline", |s| {
        s.outline() == outline(&[("alpha", &["from the popup", "two"])])
    });

    // The editor got the rendered outline, then the help comments.
    let text = std::fs::read_to_string(&seen).unwrap();
    assert!(
        text.starts_with("/1 alpha:\n  - /1 one\n  - /2 two\n"),
        "{text}"
    );
    assert!(text.contains("\n# herdr-oil:"), "{text}");

    // `edit` opened the popup and `apply` did the work; both succeed.
    server.wait_until("both actions to finish", |s| {
        let logs = s.call("plugin.log.list", json!({ "plugin_id": PLUGIN_ID }));
        let runs: Vec<(String, String)> = logs["logs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|log| {
                (
                    log["action_id"].as_str().unwrap_or_default().to_owned(),
                    log["status"].as_str().unwrap_or_default().to_owned(),
                )
            })
            .collect();
        assert!(!runs.iter().any(|(_, status)| status == "failed"), "{logs}");
        runs == [
            ("edit".to_owned(), "succeeded".to_owned()),
            ("apply".to_owned(), "succeeded".to_owned()),
        ]
    });

    let _ = std::fs::remove_dir_all(dir);
}
