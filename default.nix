with (import <nixpkgs> {});
let
  pb = import ./pb;
  generatedBuild = callPackage ./Cargo.nix {
    defaultCrateOverrides = pkgs.defaultCrateOverrides // {
      "yscloud-linker" = attrs: {
        buildInputs = [pkgs.libseccomp];
      };
      "webserver-hello-world" = attrs: {
        CERTIFICATE_ISSUER_PATH="${pb.certificateIssuer}";
        PROTOBUF_LOCATION="${pkgs.protobuf}";
        PROTOC="${pkgs.protobuf}/bin/protoc";
        PROTOC_INCLUDE="${pkgs.protobuf}/include";
        buildInputs = [pkgs.protobuf];
      };
    };
  };
  crate2nix = generatedBuild.workspaceMembers."yscloud-linker".build.override {
    runTests = true;
    # testInputs = [ pkgs.cowsay ];
  };
in generatedBuild