{
  description = "Sparksmith — development shell with the pinned Rust toolchain for `cargo xtask ci`";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # The exact toolchain in rust-toolchain.toml, including clippy, rustfmt and the
    # wasm32 target that `cargo xtask ci` checks. nixpkgs' rustc has no wasm target.
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forAllSystems =
        f:
        nixpkgs.lib.genAttrs systems (
          system:
          f (
            import nixpkgs {
              inherit system;
              overlays = [ rust-overlay.overlays.default ];
            }
          )
        );

      # Linux only (adapted from Solidcraft's flake): winit dlopens Wayland/X11, wgpu
      # loads EGL/Vulkan and rfd reaches the desktop portal through libdbus. macOS
      # uses AppKit and Metal and needs none of these.
      runtimeLibs =
        pkgs:
        pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux (
          with pkgs;
          [
            wayland
            libxkbcommon
            libglvnd
            vulkan-loader
            dbus
            libx11
            libxcursor
            libxi
            libxrandr
            libxrender
            libxext
            libxcb
          ]
        );
    in
    {
      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          name = "sparksmith";
          packages = [
            (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml)
            pkgs.git
          ]
          ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.pkg-config ];
          LD_LIBRARY_PATH = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux (
            "${pkgs.lib.makeLibraryPath (runtimeLibs pkgs)}:/run/opengl-driver/lib"
          );
          # A separate target directory keeps this toolchain's cache apart from rustup's.
          CARGO_TARGET_DIR = "target/nix";
          shellHook = ''
            echo "sparksmith: $(rustc --version); run cargo xtask ci"
          '';
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
