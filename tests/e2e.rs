//! Scenario tests: one outline edit each, against a real headless herdr.

mod common;

use serde_json::json;

use common::{Server, id_of, outline, render_ids};

#[test]
#[ignore = "needs a herdr binary in HERDR_OIL_E2E_HERDR"]
fn moves_a_split_tab_to_another_workspace_with_its_layout_and_processes() {
    let mut server = Server::start();
    let (a, a_tabs) = server.workspace("alpha", &["split", "stays"]);
    let (b, b_tabs) = server.workspace("beta", &["existing"]);

    // split: p1 | (p2 over p3), 30/70 then 60/40.
    let p1 = server.root_pane(&a_tabs[0]);
    let p2 = server.split(&p1, "right", 0.3);
    server.split(&p2, "down", 0.6);
    let before_shape = server.shape(&a_tabs[0]);
    server.call(
        "pane.send_text",
        json!({ "pane_id": p2, "text": "echo still-alive\r" }),
    );
    server.wait_for_output(&p2, "still-alive");

    let before = server.snapshot();
    server.edit(
        &before,
        &format!(
            "/{a} alpha:\n  - /{a1} stays\n\n/{b} beta:\n  - /{b0} existing\n  - /{a0} split\n",
            a1 = a_tabs[1],
            b0 = b_tabs[0],
            a0 = a_tabs[0],
        ),
    );

    assert_eq!(
        server.outline(),
        outline(&[("alpha", &["stays"]), ("beta", &["existing", "split"])])
    );
    let moved = server.snapshot().workspaces[1].tabs[1].id.clone();
    assert_eq!(server.shape(&moved), before_shape);

    let layout = server.call("layout.export", json!({ "tab_id": moved }));
    let moved_p2 = id_of(&layout["layout"]["root"]["second"]["first"]["pane_id"]);
    server.call(
        "pane.send_text",
        json!({ "pane_id": moved_p2, "text": "echo same-shell\r" }),
    );
    server.wait_for_output(&moved_p2, "still-alive");
    server.wait_for_output(&moved_p2, "same-shell");
}

#[test]
#[ignore = "needs a herdr binary in HERDR_OIL_E2E_HERDR"]
fn deep_layouts_survive_a_move() {
    let mut server = Server::start();
    let (_, a_tabs) = server.workspace("alpha", &["deep", "other"]);
    let (b, b_tabs) = server.workspace("beta", &["b"]);

    // A spiral of five nested splits in alternating directions, plus one off
    // the first pane, so the tree is not just a chain.
    let first = server.root_pane(&a_tabs[0]);
    let mut pane = first.clone();
    for (i, ratio) in [0.25, 0.4, 0.55, 0.7, 0.35].iter().enumerate() {
        let direction = if i % 2 == 0 { "right" } else { "down" };
        pane = server.split(&pane, direction, *ratio);
    }
    server.split(&first, "down", 0.5);
    let before_shape = server.shape(&a_tabs[0]);

    let before = server.snapshot();
    let text = render_ids(&before)
        .replace(&format!("  - /{} deep\n", a_tabs[0]), "")
        .replace(
            &format!("  - /{} b\n", b_tabs[0]),
            &format!("  - /{} b\n  - /{} deep\n", b_tabs[0], a_tabs[0]),
        );
    server.edit(&before, &text);

    let moved = server.snapshot().workspace(&b).unwrap().tabs[1].id.clone();
    assert_eq!(server.shape(&moved), before_shape);
}

#[test]
#[ignore = "needs a herdr binary in HERDR_OIL_E2E_HERDR"]
fn renames_creates_closes_and_reorders() {
    let mut server = Server::start();
    let (a, a_tabs) = server.workspace("alpha", &["x", "y", "z"]);
    let (b, b_tabs) = server.workspace("beta", &["q"]);

    let before = server.snapshot();
    let (report, _) = server.edit(
        &before,
        &format!(
            "/{b} renamed beta:\n  - /{q} q\n  - new one\n\n/{a} alpha:\n  - /{z} zz\n  - /{x} x\n\nfresh:\n  - brand new\n",
            q = b_tabs[0],
            z = a_tabs[2],
            x = a_tabs[0],
        ),
    );
    assert!(report.done.iter().any(|line| line.starts_with("close tab")));

    assert_eq!(
        server.outline(),
        outline(&[
            ("renamed beta", &["q", "new one"]),
            ("alpha", &["zz", "x"]),
            ("fresh", &["brand new"]),
        ])
    );
}

#[test]
#[ignore = "needs a herdr binary in HERDR_OIL_E2E_HERDR"]
fn new_workspace_from_a_moved_tab_and_deleted_workspace_closes() {
    let mut server = Server::start();
    let (a, a_tabs) = server.workspace("alpha", &["keep", "leave"]);
    let (_, b_tabs) = server.workspace("beta", &["doomed", "rescued"]);

    let before = server.snapshot();
    server.edit(
        &before,
        &format!(
            "/{a} alpha:\n  - /{keep} keep\n\ngamma:\n  - /{leave} leave\n  - /{rescued} rescued\n",
            keep = a_tabs[0],
            leave = a_tabs[1],
            rescued = b_tabs[1],
        ),
    );

    assert_eq!(
        server.outline(),
        outline(&[("alpha", &["keep"]), ("gamma", &["leave", "rescued"])])
    );
}

#[test]
#[ignore = "needs a herdr binary in HERDR_OIL_E2E_HERDR"]
fn a_workspace_emptied_into_a_new_one_closes_on_its_own() {
    let mut server = Server::start();
    let (_, a_tabs) = server.workspace("alpha", &["one", "two"]);
    let (b, b_tabs) = server.workspace("beta", &["b"]);

    let before = server.snapshot();
    let (report, _) = server.edit(
        &before,
        &format!(
            "/{b} beta:\n  - /{b0} b\n\ngamma:\n  - /{t1} one\n  - /{t2} two\n",
            b0 = b_tabs[0],
            t1 = a_tabs[0],
            t2 = a_tabs[1],
        ),
    );
    assert!(
        !report.done.iter().any(|l| l.starts_with("close")),
        "{:?}",
        report.done
    );
    assert_eq!(
        server.outline(),
        outline(&[("beta", &["b"]), ("gamma", &["one", "two"])])
    );
}

#[test]
#[ignore = "needs a herdr binary in HERDR_OIL_E2E_HERDR"]
fn a_workspace_keeping_only_a_new_tab_is_not_closed_by_the_move_out() {
    let mut server = Server::start();
    let (a, a_tabs) = server.workspace("alpha", &["only"]);
    let (b, b_tabs) = server.workspace("beta", &["b"]);

    let before = server.snapshot();
    server.edit(
        &before,
        &format!(
            // beta first, so the move out is reached before alpha's new tab.
            "/{b} beta:\n  - /{b0} b\n  - /{only} only\n\n/{a} alpha:\n  - fresh\n",
            b0 = b_tabs[0],
            only = a_tabs[0],
        ),
    );
    assert_eq!(
        server.outline(),
        outline(&[("beta", &["b", "only"]), ("alpha", &["fresh"])])
    );
}

#[test]
#[ignore = "needs a herdr binary in HERDR_OIL_E2E_HERDR"]
fn swaps_tabs_between_workspaces() {
    let mut server = Server::start();
    let (a, a_tabs) = server.workspace("alpha", &["a1", "a2"]);
    let (b, b_tabs) = server.workspace("beta", &["b1", "b2"]);

    let before = server.snapshot();
    server.edit(
        &before,
        &format!(
            "/{a} alpha:\n  - /{a1} a1\n  - /{b1} b1\n\n/{b} beta:\n  - /{a2} a2\n  - /{b2} b2\n",
            a1 = a_tabs[0],
            a2 = a_tabs[1],
            b1 = b_tabs[0],
            b2 = b_tabs[1],
        ),
    );
    assert_eq!(
        server.outline(),
        outline(&[("alpha", &["a1", "b1"]), ("beta", &["a2", "b2"])])
    );
}

#[test]
#[ignore = "needs a herdr binary in HERDR_OIL_E2E_HERDR"]
fn reverses_many_tabs_and_workspaces() {
    let mut server = Server::start();
    let labels: Vec<String> = (0..12).map(|i| format!("tab {i}")).collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    server.workspace("alpha", &refs);
    for name in ["beta", "gamma", "delta"] {
        server.workspace(name, &["x"]);
    }

    let before = server.snapshot();
    let mut reversed = before.clone();
    reversed.workspaces.reverse();
    for workspace in &mut reversed.workspaces {
        workspace.tabs.reverse();
    }
    server.edit(&before, &render_ids(&reversed));

    let expected_tabs: Vec<&str> = refs.iter().rev().copied().collect();
    assert_eq!(
        server.outline(),
        outline(&[
            ("delta", &["x"]),
            ("gamma", &["x"]),
            ("beta", &["x"]),
            ("alpha", &expected_tabs),
        ])
    );
}

#[test]
#[ignore = "needs a herdr binary in HERDR_OIL_E2E_HERDR"]
fn the_focused_tab_and_a_zoomed_tab_move_like_any_other() {
    let mut server = Server::start();
    let (a, a_tabs) = server.workspace("alpha", &["focused", "zoomed", "stays"]);
    let (b, b_tabs) = server.workspace("beta", &["b"]);

    server.call("workspace.focus", json!({ "workspace_id": a }));
    server.call("tab.focus", json!({ "tab_id": a_tabs[0] }));
    let p1 = server.root_pane(&a_tabs[1]);
    let p2 = server.split(&p1, "right", 0.4);
    server.call("pane.zoom", json!({ "pane_id": p2, "mode": "toggle" }));
    let zoomed_shape = server.shape(&a_tabs[1]);

    let before = server.snapshot();
    let text = render_ids(&before)
        .replace(&format!("  - /{} focused\n", a_tabs[0]), "")
        .replace(&format!("  - /{} zoomed\n", a_tabs[1]), "")
        .replace(
            &format!("  - /{} b\n", b_tabs[0]),
            &format!(
                "  - /{} b\n  - /{} focused\n  - /{} zoomed\n",
                b_tabs[0], a_tabs[0], a_tabs[1]
            ),
        );
    server.edit(&before, &text);

    assert_eq!(
        server.outline(),
        outline(&[("alpha", &["stays"]), ("beta", &["b", "focused", "zoomed"])])
    );
    let moved = server.snapshot().workspace(&b).unwrap().tabs[2].id.clone();
    assert_eq!(server.shape(&moved), zoomed_shape);
    // Still zoomed, on the pane that was zoomed: the second one.
    let layout = server.call("layout.export", json!({ "tab_id": moved }))["layout"].take();
    assert_eq!(layout["zoomed"], json!(true));
    assert_eq!(
        layout["focused_pane_id"],
        layout["root"]["second"]["pane_id"]
    );
}

#[test]
#[ignore = "needs a herdr binary in HERDR_OIL_E2E_HERDR"]
fn tab_names_that_are_numbers() {
    let mut server = Server::start();
    let (a, a_tabs) = server.workspace("alpha", &["first", "7", "3", "keep"]);
    let auto = id_of(&server.call("tab.create", json!({ "workspace_id": a }))["tab"]["tab_id"]);
    let (b, b_tabs) = server.workspace("beta", &["b"]);
    assert_eq!(server.snapshot().tab(&auto).unwrap().1.label, "5");

    let before = server.snapshot();
    server.edit(
        &before,
        &format!(
            "/{a} alpha:\n  - /{first} first\n  - /{keep} keep\n\n/{b} beta:\n  - /{b0} b\n  - /{seven} 7\n  - /{three} 3\n  - /{auto} 5\n",
            first = a_tabs[0],
            seven = a_tabs[1],
            three = a_tabs[2],
            keep = a_tabs[3],
            b0 = b_tabs[0],
        ),
    );

    // "7" was not at position 7, so it was a name and moves with the tab.
    // "3" sat at position 3, indistinguishable from herdr's own numbering, so
    // it is left to herdr, which renumbers it: a known limitation. The tab
    // herdr named "5" is renumbered too, as intended.
    assert_eq!(
        server.outline(),
        outline(&[
            ("alpha", &["first", "keep"]),
            ("beta", &["b", "7", "3", "4"])
        ])
    );
}

#[test]
#[ignore = "needs a herdr binary in HERDR_OIL_E2E_HERDR"]
fn odd_labels_round_trip() {
    let mut server = Server::start();
    let (a, a_tabs) = server.workspace("alpha", &["a", "b", "c", "d", "e"]);
    let odd = [
        "日本語 タブ",
        "fix: the # looks like YAML",
        "- looks like a tab",
        "/tmp/scratch",
        "quotes \" and \\ backslashes",
    ];

    let before = server.snapshot();
    let mut text = render_ids(&before).replace(
        &format!("/{a} alpha:"),
        &format!("/{a} ✓ workspace with ünïcode:"),
    );
    // Typed the way YAML wants them: quoted, since some would not read back
    // plain. A JSON string is a YAML double-quoted scalar.
    for (tab, (old, new)) in a_tabs.iter().zip(["a", "b", "c", "d", "e"].iter().zip(odd)) {
        let quoted = serde_json::to_string(&format!("/{tab} {new}")).unwrap();
        text = text.replace(&format!("  - /{tab} {old}\n"), &format!("  - {quoted}\n"));
    }
    server.edit(&before, &text);

    assert_eq!(
        server.outline(),
        outline(&[("✓ workspace with ünïcode", &odd)])
    );
    // And the rendered outline parses back to no change.
    let after = server.snapshot();
    assert_eq!(
        server.edit(&after, &render_ids(&after)).0.done,
        Vec::<String>::new()
    );
}

#[test]
#[ignore = "needs a herdr binary in HERDR_OIL_E2E_HERDR"]
fn changes_made_while_the_editor_was_open_are_respected() {
    let mut server = Server::start();
    let (a, a_tabs) = server.workspace("alpha", &["renamed", "closed", "wandered"]);
    let (b, _) = server.workspace("beta", &["b"]);

    let before = server.snapshot();
    let text = render_ids(&before).replace(
        &format!("  - /{} renamed\n", a_tabs[0]),
        &format!("  - /{} was renamed\n", a_tabs[0]),
    );

    // Meanwhile, elsewhere: a tab closes, a tab opens, a tab moves.
    server.call("tab.close", json!({ "tab_id": a_tabs[1] }));
    server.call("tab.create", json!({ "workspace_id": a, "label": "late" }));
    let wandered = server.root_pane(&a_tabs[2]);
    server.call(
        "pane.move",
        json!({ "pane_id": wandered, "destination": { "type": "new_tab", "workspace_id": b, "label": "wandered" } }),
    );

    let (_, warnings) = server.edit(&before, &text);

    // A moved tab gets a new id, so to the outline "wandered" closed too, and
    // its new tab in beta is left alone like "late".
    let mut closed: Vec<&str> = warnings
        .iter()
        .filter(|w| w.contains("closed while you were editing"))
        .map(|w| w.split_whitespace().nth(1).unwrap())
        .collect();
    closed.sort_unstable();
    assert_eq!(closed, ["\"closed\"", "\"wandered\""], "{warnings:?}");
    assert_eq!(
        server.outline(),
        outline(&[
            ("alpha", &["was renamed", "late"]),
            ("beta", &["b", "wandered"])
        ])
    );
}

#[test]
#[ignore = "needs a herdr binary in HERDR_OIL_E2E_HERDR"]
fn unchanged_outline_changes_nothing() {
    let mut server = Server::start();
    server.workspace("alpha", &["one", "2"]);
    server.workspace("beta", &["three"]);

    let before = server.snapshot();
    let (report, warnings) = server.edit(&before, &render_ids(&before));
    assert_eq!(report.done, Vec::<String>::new());
    assert_eq!(warnings, Vec::<String>::new());
    assert_eq!(server.snapshot(), before);
}

#[test]
#[ignore = "needs a herdr binary in HERDR_OIL_E2E_HERDR"]
fn a_new_tab_is_focused_even_in_another_workspace() {
    let mut server = Server::start();
    let (a, a_tabs) = server.workspace("alpha", &["here"]);
    let (b, b_tabs) = server.workspace("beta", &["there"]);
    server.call("tab.focus", json!({ "tab_id": a_tabs[0] }));

    let before = server.snapshot();
    server.edit(
        &before,
        &format!(
            "/{a} alpha:\n  - /{here} here\n\n/{b} beta:\n  - /{there} there\n  - fresh\n",
            here = a_tabs[0],
            there = b_tabs[0],
        ),
    );

    let snapshot = server.call("session.snapshot", json!({}));
    let after = server.snapshot();
    let fresh = &after.workspace(&b).unwrap().tabs[1];
    assert_eq!(fresh.label, "fresh");
    assert_eq!(
        snapshot["snapshot"]["focused_tab_id"].as_str(),
        Some(fresh.id.as_str()),
        "{snapshot}"
    );
}
