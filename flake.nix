{
  description = "k9s-gpui: a native GPUI client replicating k9s";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      inherit (nixpkgs) lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems =
        f:
        lib.genAttrs systems (
          system:
          f (
            import nixpkgs {
              inherit system;
              overlays = [ rust-overlay.overlays.default ];
            }
          )
        );
    in
    {
      devShells = forAllSystems (pkgs: 
        let
          toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          runtimeLibs = with pkgs; [
            wayland
            libxkbcommon
            vulkan-loader
            vulkan-tools
            libx11
            libxcb
            libxcursor
            libxrandr
            libxi
            fontconfig
            freetype
            openssl
          ];
        in
        {
          default = pkgs.mkShell {
            nativeBuildInputs = with pkgs; [
              toolchain
              pkg-config
              cmake
              mold
              clang
              go-task
              cargo-nextest
              kubectl
              kind
            ];
            buildInputs = runtimeLibs ++ [ pkgs.vulkan-headers ];

            LD_LIBRARY_PATH = lib.makeLibraryPath runtimeLibs + ":/run/opengl-driver/lib";
            RUST_FONTCONFIG_DLOPEN = "1";

            shellHook = ''
              echo "k9s-gpui dev shell: $(rustc --version)"
            '';
          };
        }
      );

      formatter = forAllSystems (pkgs: pkgs.nixfmt-rfc-style);
    };
}
