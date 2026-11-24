{
  description = "herdr-oil checks and dev shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # The herdr the end-to-end tests run against; keep at the manifest's
    # min_herdr_version or newer.
    herdr = {
      url = "github:herdrdev/herdr/v0.9.5";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # VHS with animated SVG output, for the README demos (demo/README.md).
    vhs = {
      url = "github:agentstation/vhs/36343ddb468785b82ea92f17db841eae7cfe3e2f";
      flake = false;
    };
  };

  outputs =
    {
      herdr,
      nixpkgs,
      vhs,
      ...
    }:
    let
      systems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];
      forAllSystems =
        f:
        nixpkgs.lib.genAttrs systems (
          system:
          f {
            inherit system;
            pkgs = nixpkgs.legacyPackages.${system};
            herdrPackage = herdr.packages.${system}.default;
            herdrBin = "${herdr.packages.${system}.default}/bin/herdr";
          }
        );
      package = pkgs: pkgs.callPackage ../default.nix { };
    in
    {
      packages = forAllSystems (
        { herdrPackage, pkgs, ... }:
        let
          # Font subsetting for the SVG's embedded @font-face (vhs PR #8).
          pyftsubset = pkgs.python3Packages.fonttools.overridePythonAttrs (old: {
            dependencies = (old.dependencies or [ ]) ++ [ pkgs.python3Packages.brotli ];
          });

          # agentstation's VHS fork with its open SVG PRs applied.
          vhsSvg = pkgs.buildGoModule {
            pname = "vhs";
            version = "0.11.1-svg";
            src = vhs;
            patches = [
              # PR #6: wall-clock timestamps for SVG frame capture
              (builtins.fetchurl {
                url = "https://github.com/agentstation/vhs/pull/6.patch";
                sha256 = "0y364q2rkvh3ny6ng0gp61r934k26h287mxwxdrk6l8hfr3nmarx";
              })
              # PR #7: snap SVG dimensions to terminal character grid
              (builtins.fetchurl {
                url = "https://github.com/agentstation/vhs/pull/7.patch";
                sha256 = "00zj6bcs2gn1dblgbakh9gdiaccwic7d94d7zgqb771ydj6bzgnp";
              })
              # PR #8: embed font via @font-face with pyftsubset subsetting
              (builtins.fetchurl {
                url = "https://github.com/agentstation/vhs/pull/8.patch";
                sha256 = "0la8p3pvwb171ayxjnw97ipr99lk5iy5v13nrag08mvrwcpmch9a";
              })
              # PR #9: add progress bar overlay to SVG output
              (builtins.fetchurl {
                url = "https://github.com/agentstation/vhs/pull/9.patch";
                sha256 = "05z2ilrvg6bjn7f1mfq0dm907mx8lpj2y6xd49b4zxy2i5m4xmzb";
              })
            ];
            vendorHash = "sha256-WiCSn84cr42yQFgg36H/NrVsfiBA/ZDAGd0WmC6LAa4=";
            nativeBuildInputs = [ pkgs.makeWrapper ];
            postInstall = ''
              wrapProgram $out/bin/vhs --prefix PATH : ${
                pkgs.lib.makeBinPath [
                  pkgs.ffmpeg
                  pkgs.fontconfig
                  pkgs.ttyd
                  pyftsubset
                ]
              }
            '';
            doCheck = false;
            meta.mainProgram = "vhs";
          };
        in
        {
          default = package pkgs;

          vhs-svg = vhsSvg;

          # `nix run ./dev#build-demo [demo/name.tape ...]` from the repo root.
          build-demo = pkgs.writeShellApplication {
            name = "build-demo";
            runtimeInputs = [
              herdrPackage
              # The panes' shell; plain bash has no readline for the prompt.
              pkgs.bashInteractive
              pkgs.coreutils
              pkgs.jq
              pkgs.neovim
              pkgs.python3
              pkgs.svgo
            ];
            text = ''
              export HERDR_OIL_BIN=${package pkgs}/bin/herdr-oil
              export HERDR_OIL_DEMO_PATH=$PATH
              export FONTCONFIG_FILE=${pkgs.makeFontsConf { fontDirectories = [ pkgs.maple-mono.NF ]; }}
              budget_kb=''${DEMO_BUDGET_KB:-600}
              [ -f demo/session.sh ] || { echo "run from the herdr-oil repo root" >&2; exit 1; }
              if [ "$#" -eq 0 ]; then
                for tape in demo/*.tape; do
                  [ "$tape" = demo/settings.tape ] || set -- "$@" "$tape"
                done
              fi
              status=0
              for tape in "$@"; do
                name=$(basename "$tape" .tape)
                ${pkgs.lib.getExe vhsSvg} "$tape"
                svgo --config ${./svgo.config.mjs} --input "demo/$name.svg" --output "demo/$name.svg"
                # The backgrounds sit on the terminal's cell grid, but the text
                # flows at the font's advance (0.6em in Maple Mono), which is
                # narrower, so text drifts off its backgrounds along a row.
                # Spacing the letters out to the cell width lines them up;
                # the terminal drew no ligatures, so the SVG shouldn't either.
                cell=$(grep -o 'h[0-9.]*v' "demo/$name.svg" | sort | uniq -c | sort -rn | awk 'NR == 1 { print substr($2, 2, length($2) - 2) }')
                size=$(grep -o 'font-size:[0-9.]*px' "demo/$name.svg" | head -1 | tr -dc '0-9.')
                spacing=$(awk -v c="$cell" -v s="$size" 'BEGIN { print c - 0.6 * s }')
                sed -i "s/font-size:''${size}px}/font-size:''${size}px;letter-spacing:''${spacing}px;font-variant-ligatures:none}/" "demo/$name.svg"
                grep -q 'letter-spacing' "demo/$name.svg" || { echo "demo/$name.svg: could not fix letter spacing" >&2; exit 1; }
                # After the spacing, which reads the cell width off the cells.
                python3 -I ${./merge-cells.py} "demo/$name.svg"
                kb=$(( $(stat -c %s "demo/$name.svg") / 1024 ))
                echo "demo/$name.svg: $kb KB (budget $budget_kb KB)"
                if [ "$kb" -gt "$budget_kb" ]; then
                  echo "demo/$name.svg is over budget; shorten the tape" >&2
                  status=1
                fi
              done
              exit "$status"
            '';
          };

          # `nix run ./dev#upload-demo` from the repo root, with the R2 bucket's
          # S3 credentials in AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY.
          upload-demo = pkgs.writeShellApplication {
            name = "upload-demo";
            runtimeInputs = [ pkgs.awscli2 ];
            text = ''
              [ -f demo/session.sh ] || { echo "run from the herdr-oil repo root" >&2; exit 1; }
              aws s3 sync demo/ "s3://''${R2_BUCKET:-herdr-oil}/" \
                --endpoint-url https://946b8d18ae9ef27fc85597e7716a1641.r2.cloudflarestorage.com \
                --exclude "*" --include "*.svg" --content-type image/svg+xml
              echo "Uploaded to https://herdr-oil.xav.ie/; bump ?v= in README.md for changed demos"
            '';
          };
        }
      );

      checks = forAllSystems (
        { herdrBin, pkgs, ... }:
        let
          herdr-oil = package pkgs;
          # The package's own sources and vendored deps, with a different phase.
          cargoCheck =
            name: nativeBuildInputs: command:
            herdr-oil.overrideAttrs (old: {
              name = "herdr-oil-${name}";
              nativeBuildInputs = old.nativeBuildInputs ++ nativeBuildInputs;
              buildPhase = command;
              doCheck = false;
              installPhase = "touch $out";
              postInstall = "";
            });
        in
        {
          build = herdr-oil;

          clippy = cargoCheck "clippy" [ pkgs.clippy ] ''
            cargo clippy --offline --all-targets -- -D warnings
          '';

          # Unit tests, then the tests against a real herdr, including a real
          # client in tmux editing in vim.
          tests = herdr-oil.overrideAttrs (old: {
            name = "herdr-oil-tests";
            nativeCheckInputs = (old.nativeCheckInputs or [ ]) ++ [
              pkgs.tmux
              pkgs.vim
            ];
            HERDR_OIL_E2E_HERDR = herdrBin;
            cargoTestFlags = [
              "--"
              "--include-ignored"
            ];
          });

          formatting =
            pkgs.runCommand "herdr-oil-formatting"
              {
                nativeBuildInputs = [
                  pkgs.cargo
                  pkgs.nixfmt
                  pkgs.rustfmt
                ];
              }
              ''
                cd ${../.}
                cargo fmt --check
                nixfmt --check flake.nix default.nix dev/flake.nix
                touch $out
              '';
        }
      );

      devShells = forAllSystems (
        { herdrBin, pkgs, ... }:
        {
          default = pkgs.mkShell {
            inputsFrom = [ (package pkgs) ];
            packages = [
              pkgs.clippy
              # Release notes: knope document-change.
              pkgs.knope
              pkgs.nixfmt
              pkgs.rust-analyzer
              pkgs.rustfmt
              pkgs.tmux
              pkgs.vim
            ];
            HERDR_OIL_E2E_HERDR = herdrBin;
          };
        }
      );
    };
}
