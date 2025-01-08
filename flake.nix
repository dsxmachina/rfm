{
  description = "rfm - rusty file manager";

  inputs = {
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.follows = "rust-overlay/flake-utils";
    nixpkgs.follows = "rust-overlay/nixpkgs";
    # add compatibility layer
    flake-compat.url = "https://flakehub.com/f/edolstra/flake-compat/1.tar.gz";
  };
  
  outputs = inputs: with inputs;
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        inherit (pkgs) lib;
        # Will be reused in the dev-shell section
        commonArgs = {
          nativeBuildInputs = with pkgs; [];
          buildInputs = with pkgs; [] ++ lib.optionals pkgs.stdenv.isDarwin [
            pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
            pkgs.libiconv
          ];
        };
        # This is necessary, since we import these libraries directly via git in cargo

        # Define the rfm package
        rfm = pkgs.rustPlatform.buildRustPackage {
          pname = "rfm";
          version = "0.3.0";
          src = ./.;
          cargoLock = { 
            lockFile = ./Cargo.lock; 
          };

          # use the above build-intputs
          inherit (commonArgs) nativeBuildInputs buildInputs;
        };
      in rec {
        packages.default = rfm;
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            cargo-edit
            cargo-audit
            cargo-tarpaulin
            cargo-machete
            cargo-bloat
            cargo-hakari
            cargo-nextest
            cargo-deny
            clippy
            rustfmt
          ] ++ commonArgs.nativeBuildInputs ++ commonArgs.buildInputs;

          # Certain Rust tools won't work without this
          # This can also be fixed by using oxalica/rust-overlay and specifying the rust-src extension
          # See https://discourse.nixos.org/t/rust-src-not-found-and-other-misadventures-of-developing-rust-on-nixos/11570/3?u=samuela. for more details.
          RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
        };
      }
    );
}
