{ ... }:
let
  pkgs = import ./default.nix;
  setMerge = (a: b: a // b);
  builtCrate = x: {
    # binary/library
    "${x}" = pkgs.workspaceMembers."${x}".build;
    # and the tests
    "${x}-test" = pkgs.workspaceMembers."${x}".build.override {
      runTests = true;
    };
  };
in builtins.foldl' setMerge {}
  (map builtCrate (builtins.attrNames pkgs.workspaceMembers))
