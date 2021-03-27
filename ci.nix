{ ... }:
let
  pkgs = import ./default.nix;
in {
  allWorkspaceMembers = pkgs.allWorkspaceMembers;
}
