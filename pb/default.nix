with (import <nixpkgs> {});
let
  mkDerivation = import ./nix/pb.nix pkgs;
  parent = import ../Cargo.nix {};
in
{
  certificateIssuer = mkDerivation {
    name = "certificate_issuer";
    buildInputs = with pkgs; [coreutils protobuf];
    src = ./certificate_issuer;

    PROTOBUF_LOCATION="${pkgs.protobuf}";
    PROTOC="${pkgs.protobuf}/bin/protoc";
    PROTOC_INCLUDE="${pkgs.protobuf}/include";
    BUILDER="${parent.workspaceMembers."pb-builder".build}/bin/pb-builder";
  };
}
