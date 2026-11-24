# herdr-oil

Reorganize [herdr](https://herdr.dev) workspaces and tabs by editing a text
file, the way [oil.nvim](https://github.com/stevearc/oil.nvim) edits a
directory. A keybinding opens every workspace and tab as a YAML outline in your
`$EDITOR`, in a popup. Move, reorder, rename, add or delete lines, then save
and quit, and herdr is rearranged to match.

<p align="center">
  <img src="https://herdr-oil.xav.ie/move.svg?v=14" width="100%" alt="Moving the current tab, split and running server included, to another workspace">
  <br>
  <em>Move a tab to another workspace.</em>
</p>

```diff
 /1 notes:
-  - /1 server
   - /2 scratch
   - /3 old experiment

 /2 api:
   - /4 tests
   - /5 fix flaky auth test
+  - /1 server
```

| Edit | Result |
|---|---|
| Move a `- tab` line under another `workspace:` | The tab moves there, its split layout and running processes intact |
| Reorder lines | Tabs and workspaces reorder |
| Change the text after the number | Rename |
| Add a line without a number | Create a tab or workspace |
| Copy a tab line | Create a new tab with that name |
| Delete a line | Close it, after a confirmation that lists what dies |

When it is done, a toast says what changed. A tab you added gets the focus,
as in a browser; otherwise you stay on the pane you opened the outline from,
wherever its tab went. Quit without saving to cancel. Since the file is YAML, your editor highlights
and folds it; a name containing `: ` or ` #` is written in double quotes so it
stays valid. Edits are read the way they were meant where that is clear: a
line missing its `-` or `:` is a tab when indented and a workspace when not, a
number that stands for nothing starts a new item, and a workspace left without
tabs gets a new one. Only what cannot be read, such as a tab above every
workspace or an unclosed quote, sends you back to the editor, with the problem
as a comment above its line.

<p align="center">
  <img src="https://herdr-oil.xav.ie/create.svg?v=4" width="100%" alt="Adding a line under a workspace, then saving">
  <br>
  <em>Add a tab.</em>
</p>

```diff
 /1 notes:
   - /1 server
   - /2 scratch
   - /3 old experiment

 /2 api:
   - /4 tests
   - /5 fix flaky auth test
+  - review PR 42
```

<p align="center">
  <img src="https://herdr-oil.xav.ie/close.svg?v=6" width="100%" alt="Deleting a tab's line, saving, and confirming the close">
  <br>
  <em>Close a tab, once you confirm.</em>
</p>

```diff
 /1 notes:
   - /1 server
   - /2 scratch
-  - /3 old experiment

 /2 api:
   - /4 tests
   - /5 fix flaky auth test
```

## Install

With herdr's plugin manager, which builds it with `cargo`:

```sh
herdr plugin install xav-ie/herdr-oil
```

That installs the latest commit on `main`. To install a release instead, pin its
tag (see [Releases](https://github.com/xav-ie/herdr-oil/releases)):

```sh
herdr plugin install xav-ie/herdr-oil --ref v0.1.0
```

Then bind the action in `~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+o"
type = "plugin_action"
command = "xav-ie.herdr-oil.edit"
description = "Edit workspaces and tabs"
```

### Nix

The flake has no inputs; its overlay adds `herdr-oil`, built with your
nixpkgs. The package carries a ready plugin root at
`share/herdr-oil` (also `passthru.pluginRoot`) for a `plugins.json` entry with
`source.kind = "local"`.

```nix
{
  inputs.herdr-oil.url = "github:xav-ie/herdr-oil";
  # nixpkgs.overlays = [ inputs.herdr-oil.overlays.default ];
}
```

## How it works

Each line starts with the number of the workspace or tab it was, which is how
an edited line is traced back: workspaces count from 1 down the file, and so do
tabs, across workspaces, so a tab keeps its number when it moves. herdr keeps running while you edit, so the
changes are worked out against a fresh snapshot taken when the editor closes:
anything closed in the meantime is skipped with a warning, and tabs opened in
the meantime are left alone.

herdr moves panes, not tabs. A tab moving to another workspace is rebuilt there
pane by pane with `pane.move`, splitting each pane back in beside the one it
was next to, at the same ratio, and zoomed again if it was zoomed. Processes
keep running throughout.

A few things follow from how herdr works:

- A workspace needs at least one tab. To close one, delete its line.
- Tabs herdr named automatically (by position) keep following their position
  after a move; tabs you named keep their names.
- Panes are not listed. A pane's place is two-dimensional, and a text outline
  can't say which side it splits from or how big it is; herdr's own split and
  swap keys handle that.

## Development

In `nix develop ./dev`, `cargo test` runs the unit tests,
`cargo test -- --include-ignored` adds every test against a real herdr, and
`nix flake check ./dev` runs everything CI runs:

```sh
nix develop ./dev
cargo test -- --include-ignored
nix flake check ./dev
```

| Suite | What it covers |
|---|---|
| unit (`src/`) | Parsing, planning, layout rebuild order, and `apply` against a fake herdr: call order, failures, refused moves |
| `tests/e2e.rs` | One edit per test against a real herdr: split and deep layouts, zoom, focus, swaps, numeric and odd labels, changes made while the editor is open |
| `tests/cli.rs` | The binary: every prompt answer, re-editing, line errors, quitting without saving, `$EDITOR` with arguments, the temp file |
| `tests/plugin.rs` | The plugin linked into herdr: action, popup, editor, applied result |
| `tests/random.rs` | Random sessions and random edits, three rounds per seed, checked exactly, layouts included |

The tests against herdr start their own isolated server each (temporary
`HOME`), so they never touch your running session. They need
`HERDR_OIL_E2E_HERDR`, which the dev shell points at the herdr the dev flake
pins. `HERDR_OIL_RANDOM_CASES` sets how many random seeds run (default 40), and a
failure prints the `HERDR_OIL_RANDOM_SEED` that replays it.

To try it without installing the plugin, run `cargo run -- edit` in any herdr
pane: it edits the session that pane belongs to.

### Releasing

Each change that users would notice gets a note in `.changeset/`, written with
`knope document-change` in the dev shell and committed with the change. Every
push to `main` keeps one "chore: prepare release" pull request up to date with
the next version and its changelog; merging it tags `v<version>` and publishes
the GitHub release. The version lives in `Cargo.toml`, `Cargo.lock` and
`herdr-plugin.toml`, and knope keeps all three in step.

### Demos

The recordings in `demo/` are made with [VHS](https://github.com/agentstation/vhs)
(a fork with animated SVG output), compressed with svgo, and shrunk further by
merging runs of same-colored background cells (`dev/merge-cells.py`).

Record every `demo/*.tape`, or name the ones to record, then upload the SVGs
to the R2 bucket served at `herdr-oil.xav.ie`:

```sh
nix run ./dev#build-demo
nix run ./dev#build-demo -- demo/move.tape
nix run ./dev#upload-demo
```

The SVGs are not committed; after uploading a changed one, bump its `?v=` in
this README so GitHub's image cache fetches it again. Uploading needs the
bucket's S3 credentials in `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`.

Each tape runs `demo/session.sh`, which starts its own herdr in a temporary
home, so recording never touches your session. The build fails when an SVG
grows past `DEMO_BUDGET_KB` (default 600): every distinct screen is a full
frame, so keep tapes short and avoid output that changes on its own.

## License

MIT
