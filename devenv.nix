{ pkgs, ... }:
{
  packages = [
    pkgs.nodejs_24
    pkgs.python3
    (pkgs.rWrapper.override { packages = [ pkgs.rPackages.digest ]; })
  ];

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
    cargo test --workspace --all-targets --locked
    npm --prefix docs ci
    npm --prefix docs run deploy:check
  '';
}
