#!/usr/bin/env bash
# Starts an isolated herdr with herdr-oil linked and a believable session in
# it, then attaches a client: what a demo tape records. Run by the tapes via
# `nix run ./dev#build-demo`, which puts herdr, nvim and HERDR_OIL_BIN in place.
#
#   demo/session.sh [tab]   start on the tab named [tab] (default: server)
set -euo pipefail
# A login shell (VHS's, tmux's) rebuilds PATH from the system profile, so
# build-demo hands its PATH over in a variable of its own.
PATH=${HERDR_OIL_DEMO_PATH:-$PATH}
# Run from inside herdr, these would point every command below at that
# session instead of the demo's own.
while IFS='=' read -r var _; do
  case $var in HERDR_OIL_*) ;; HERDR_*) unset "$var" ;; esac
done < <(env)
if [ -n "${HERDR_SOCKET_PATH:-}" ]; then
  echo "refusing to run with HERDR_SOCKET_PATH set: it would drive a real session" >&2
  exit 1
fi

root=$(mktemp -d "${TMPDIR:-/tmp}/herdr-oil-demo.XXXXXX")
export HOME="$root" XDG_CONFIG_HOME="$root/config" SHELL=bash
# No ruler, pending keys, search highlight or count: each is another frame
# to record.
export EDITOR="nvim --clean -c 'set laststatus=0 nowrap cursorline noruler noshowcmd noshowmode nohlsearch noincsearch shortmess+=FISsW'"
mkdir -p "$XDG_CONFIG_HOME/herdr" "$root/plugin/target/release" "$root/src"/{api,notes}

# Read after the system bashrc, which would otherwise put the real user and
# host in every prompt.
cat > "$HOME/.bashrc" <<'BASHRC'
PS1='\[\e[35m\]\W\[\e[0m\] $ '
BASHRC

# The server tab's process: a log written once, then still running, so the
# move can be seen keeping it without the screen changing on its own.
cat > "$root/src/notes/serve" <<'SERVE'
#!/usr/bin/env bash
printf '\e[32mlistening\e[0m on :8080\n'
for ms in 4 2 7 3 5; do
  printf '\e[32mGET\e[0m /health 200 %sms\n' "$ms"
done
exec sleep infinity
SERVE
chmod +x "$root/src/notes/serve"

cat > "$XDG_CONFIG_HOME/herdr/config.toml" <<'EOF'
onboarding = false

[terminal]
default_shell = "bash"

[update]
version_check = false
manifest_check = false

# The narrowest sidebar herdr allows, leaving the recording to the panes.
[ui]
sidebar_width = 18

[ui.toast]
delivery = "herdr"

# Catppuccin Mocha, livelier: the sidebar on its own darker background, the
# current workspace on a mauve row, and the focused pane's border and current
# tab in mauve too, against borders and labels dim enough to set them off.
[theme]
name = "catppuccin"

[theme.custom]
sidebar_bg = "#11111b"
active_row_bg = "#45386b"
accent = "#cba6f7"
overlay0 = "#7f849c"
overlay1 = "#a6adc8"

[[keys.command]]
key = "prefix+o"
type = "plugin_action"
command = "xav-ie.herdr-oil.edit"
description = "Edit workspaces and tabs"
EOF

# The plugin root as the nix package lays it out.
here=$(cd "$(dirname "$0")/.." && pwd)
sed '/^\[\[build\]\]/,/^$/d' "$here/herdr-plugin.toml" > "$root/plugin/herdr-plugin.toml"
ln -s "$HERDR_OIL_BIN" "$root/plugin/target/release/herdr-oil"
herdr plugin link "$root/plugin" > /dev/null

herdr server > "$root/server.log" 2>&1 &
server=$!
trap 'kill "$server" 2> /dev/null; rm -rf "$root"' EXIT
until herdr workspace list > /dev/null 2>&1; do sleep 0.1; done

workspace() { # label dir -> sets $workspace, $first_tab, $first_pane
  local out
  out=$(herdr workspace create --label "$1" --cwd "$2" --no-focus)
  workspace=$(jq -r .result.workspace.workspace_id <<< "$out")
  first_tab=$(jq -r .result.tab.tab_id <<< "$out")
  first_pane=$(jq -r .result.root_pane.pane_id <<< "$out")
}
tab() { # workspace label
  herdr tab create --workspace "$1" --label "$2" --no-focus > /dev/null
}

# Nerd Font icons from Maple Mono NF, one cell wide and embedded in the SVG
# with the rest of the font, so it is plain which workspace a tab is in.
# `server` starts out misfiled under notes, for the move demo to put right.
workspace " notes" "$root/src/notes"
notes=$workspace server_tab=$first_tab log=$first_pane
herdr tab rename "$server_tab" "server" > /dev/null
tab "$notes" "scratch"
tab "$notes" "old experiment"

workspace " api" "$root/src/api"
herdr tab rename "$first_tab" "tests" > /dev/null
tab "$workspace" "fix flaky auth test"

# The server tab: a log on the left, a shell on the right, so a move can be
# seen keeping both.
herdr pane split "$log" --direction right --ratio 0.55 > /dev/null
herdr pane run "$log" ./serve > /dev/null

start=$(herdr tab list | jq -c --arg label "${1:-server}" '.result.tabs[] | select(.label == $label)')
herdr workspace focus "$(jq -r .workspace_id <<< "$start")" > /dev/null
herdr tab focus "$(jq -r .tab_id <<< "$start")" > /dev/null

herdr
