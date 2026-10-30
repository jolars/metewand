{ pkgs, lib, ... }:
{
  packages = [
    pkgs.cargo-nextest
    pkgs.nodejs_24
    pkgs.python3
    pkgs.uv
    (pkgs.rWrapper.override {
      packages = with pkgs.rPackages; [
        jsonlite
        roxygen2
        testthat
        withr
      ];
    })
  ];

  env.MINIFLARE_WORKERD_PATH = lib.mkIf pkgs.stdenv.hostPlatform.isLinux (
    pkgs.writeShellScript "metewand-workerd" ''
      set -eu
      workerd="$(${lib.getExe pkgs.nodejs_24} -e \
        'const { createRequire } = require("node:module");
         const wranglerRequire = createRequire(require.resolve(process.argv[1]));
         process.stdout.write(wranglerRequire("workerd").default);' \
        "$DEVENV_ROOT/docs/node_modules/wrangler")"
      exec ${pkgs.stdenv.cc.bintools.dynamicLinker} "$workerd" "$@"
    ''
  );

  scripts.test-fast.exec = ''
    cargo nextest run --test integration --locked \
      -E 'test(/^(core|core_tests|protocol|protocol_tests)::/)' "$@"
  '';

  scripts.test-rust.exec = ''
    cargo nextest run --workspace --all-targets --locked "$@"
  '';

  languages.rust = {
    enable = true;
    toolchainFile = ./rust-toolchain.toml;
  };

  git-hooks.hooks = {
    clippy = {
      enable = true;
      settings.allFeatures = true;
    };

    rustfmt.enable = true;
  };

  enterTest = ''
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
    cargo nextest run --workspace --all-targets --locked --profile ci
    npm --prefix docs ci
    npm --prefix docs run deploy:check
  '';
}
