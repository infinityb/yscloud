{
  configuration ? import ./lib/from-env.nix "NIXOS_CONFIG" <nixos-config>,
  system ? builtins.currentSystem,
  extraPackages ? [],
}:

let
  makeSquashImage = import ../nixlib/make-squashfs.nix;

  eval = import <nixpkgs/nixos/lib/eval-config.nix> {
    inherit system;
    modules = [ configuration ];
  };

  # This is for `nixos-rebuild build-vm'.
  vmConfig = (import <nixpkgs/nixos/lib/eval-config.nix> {
    inherit system;
    modules = [ configuration <nixpkgs/nixos/modules/virtualisation/qemu-vm.nix> ];
  }).config;

  # This is for `nixos-rebuild build-vm-with-bootloader'.
  vmWithBootLoaderConfig = (import <nixpkgs/nixos/lib/eval-config.nix> {
    inherit system;
    modules =
      [ configuration
        <nixpkgs/nixos/modules/virtualisation/qemu-vm.nix>
        { virtualisation.useBootLoader = true; }
      ];
  }).config;

in rec {
  inherit eval;

  toplevel = eval.config.system.build.toplevel;

  image = makeSquashImage {
    configuration = {
      packages = [ toplevel ] ++ extraPackages;
    };
  };
}

# {
#   inherit (eval) pkgs config options;

#   system = eval.config.system.build.toplevel;

#   vm = vmConfig.system.build.vm;

#   vmWithBootLoader = vmWithBootLoaderConfig.system.build.vm;

#   platformImage = makeSquashImage {
#     configuration = {
#       packages = [ eval.config.system.build.toplevel ];
#     };
#   };
# }