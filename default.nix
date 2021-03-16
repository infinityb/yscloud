with (import <nixpkgs> {});
let
  pb = import ./pb;
  iconvOptional = lib.optionals stdenv.isDarwin [pkgs.libiconv];
  darwinHack = attrs: { buildInputs = iconvOptional; };
  generatedBuild = callPackage ./Cargo.nix {
    defaultCrateOverrides = pkgs.defaultCrateOverrides // {
      "yscloud-linker" = attrs: {
        buildInputs = lib.optionals stdenv.isLinux [pkgs.libseccomp];
      };
      "appliance-init" = darwinHack;
      "ksuid-cli" = darwinHack;
      "sni-multiplexor" = darwinHack;
    };
  };
in generatedBuild
