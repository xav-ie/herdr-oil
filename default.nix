# The plugin root lands at share/herdr-oil (passthru.pluginRoot): the manifest
# without its [[build]] step, since nix already built the binary and the store
# is read-only, and the binary at the target/release path the manifest runs.
{
  lib,
  rustPlatform,
}:
rustPlatform.buildRustPackage {
  pname = "herdr-oil";
  version = (lib.importTOML ./Cargo.toml).package.version;

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.lock
      ./Cargo.toml
      ./herdr-plugin.toml
      ./src
      ./tests
    ];
  };

  cargoLock.lockFile = ./Cargo.lock;

  postInstall = ''
    root="$out/share/herdr-oil"
    install -d "$root/target/release"
    ln -s "$out/bin/herdr-oil" "$root/target/release/herdr-oil"
    sed '/^\[\[build\]\]/,/^$/d' herdr-plugin.toml > "$root/herdr-plugin.toml"
    if grep -q '^\[\[build\]\]' "$root/herdr-plugin.toml"; then
      echo "herdr-plugin.toml still has a [[build]] step" >&2
      exit 1
    fi
  '';

  passthru.pluginRoot = "share/herdr-oil";

  meta = {
    description = "Reorganize herdr workspaces and tabs by editing an outline in $EDITOR";
    homepage = "https://github.com/xav-ie/herdr-oil";
    license = lib.licenses.mit;
    mainProgram = "herdr-oil";
    platforms = lib.platforms.unix;
  };
}
