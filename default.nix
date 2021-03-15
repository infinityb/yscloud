with (import <nixpkgs> {});
let
  generatedBuild = callPackage ./Cargo.nix {
    defaultCrateOverrides = pkgs.defaultCrateOverrides // {
      "yscloud-linker" = attrs: {
        buildInputs = [pkgs.libseccomp];
      };
    };
  };
in generatedBuild.workspaceMembers