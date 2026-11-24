{
  description = "herdr-oil: reorganize herdr workspaces and tabs by editing an outline in $EDITOR";

  # No inputs: build it with your own nixpkgs through the overlay, or
  # callPackage default.nix. Checks live in ./dev.
  outputs = _: {
    overlays.default = final: _prev: {
      herdr-oil = final.callPackage ./default.nix { };
    };
  };
}
