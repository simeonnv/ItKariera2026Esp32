{
  description = "A nix-shell for developing with Rust on Xtensa+RISCV ESP32 targets";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
  };
  outputs = inputs @ {flake-parts, ...}:
    flake-parts.lib.mkFlake {inherit inputs;}
    {
      systems = ["x86_64-linux" "aarch64-linux" "aarch64-darwin"];
      imports = [
        ./package.nix
      ];

      perSystem = {
        pkgs,
        self',
        ...
      }: {
        devShells.default = pkgs.mkShell {
          packages = [
            self'.packages.esp-rs
            pkgs.rust-analyzer
            pkgs.rustup
            pkgs.espflash
            pkgs.pkg-config
            pkgs.stdenv.cc
            pkgs.mise
          ];

          shellHook = ''
            export LIBCLANG_PATH="${pkgs.llvmPackages.libclang.lib}/lib"
            export PS1="(esp-rs)$PS1"
            export RUSTUP_TOOLCHAIN=${self'.packages.esp-rs}
          '';
        };
      };
    };
}
