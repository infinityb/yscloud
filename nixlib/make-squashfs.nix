{
  pkgs ? import <nixpkgs> { system = "x86_64-linux"; },
  configuration,
}: pkgs.callPackage <nixpkgs/nixos/lib/make-squashfs.nix> {
  storeContents = configuration.packages;
  comp = "gzip -noD";
}