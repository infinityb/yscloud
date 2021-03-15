with (import <nixpkgs> {});
let
  rustSource = import ../Cargo.nix {};
  makeSquashImage = import ../lib/make-squashfs.nix;
  # kernel-name = config.boot.kernelPackages.kernel.name or "kernel";
  # 
  # modulesTree = config.system.modulesTree.override { name = kernel-name + "-modules"; };
  # firmware = config.hardware.firmware;

  # modulesClosure = pkgs.makeModulesClosure {
  #   rootModules = config.boot.initrd.availableKernelModules ++ config.boot.initrd.kernelModules;
  #   kernel = modulesTree;
  #  firmware = firmware;
  #  allowMissing = true;
  # };
  modulesClosure = pkgs.makeModulesClosure {
    rootModules = [
      # Standard hardware
      "igb" "ixgbe"

      # Qemu-compatibility
      "virtio-net" "virtio-pci"

      "squashfs"
    ];
    kernel = pkgs.linuxPackages.kernel;
    firmware = [];
  };

in rec {
  platformImage = makeSquashImage {
    configuration = {
      packages = [
        # rustSource.workspaceMembers
        pkgs.linuxPackages.kernel

        pkgs.zfs
        pkgs.linuxPackages.zfs
      ];
    };
  };

  # HACK: add ${pkgs.linuxPackages.zfs} here if we need the zfs modules
  # 
  bootStageHelper = pkgs.writeText "stage1.proplist"
    ''
      MKDIR /lib
      LINK /lib/modules ${modulesClosure}/lib/modules
      LINK /lib/systemd ${pkgs.systemdMinimal}/lib/systemd
      MKDIR /usr
      MKDIR /usr/bin
      LINK /usr/bin/mount ${pkgs.busybox}/bin/mount
      LINK /usr/bin/udevadm ${pkgs.systemdMinimal}/bin/udevadm
      MKDIR /usr/sbin
      LINK /usr/sbin/sysctl ${pkgs.busybox}/bin/sysctl
      LINK /usr/sbin/ip ${pkgs.busybox}/bin/ip
      LINK /usr/sbin/netman ${rustSource.workspaceMembers."appliance-netman".build}/bin/appliance-netman
    '';

  initrd = makeInitrd {
    name = "initrd-${pkgs.linuxPackages.kernel.name}";

    contents = [
      { object = "${rustSource.workspaceMembers."appliance-init".build}/bin/appliance-init";
        symlink = "/init";
      }
      { object = bootStageHelper;
        symlink = "/init.config";
      }
    ];
  };

  startScriptQemu = pkgs.writeText "run-vm"
    ''
    qemu-system-x86_64 \
      -m 1024 \
      -nographic -serial mon:stdio \
      -append 'console=ttyS0' \
      -netdev user,id=n1 -device virtio-net-pci,netdev=n1 \
      -kernel ${pkgs.linuxPackages.kernel}/bzImage \
      -initrd ${initrd}/initrd.gz
    '';

  startScriptFirecracker = pkgs.writeText "run-vm"
    ''
    ${pkgs.firectl}/bin/firectl -c4 -m1024 \
      --firecracker-binary=${pkgs.firectl}/bin/firecracker \
      --kernel=${pkgs.linuxPackages.kernel}/bzImage \
      --root-drive=${initrd}/initrd.gz \
      --cpu-template=T2 \
      --firecracker-log=firecracker-vmm.log \
      --kernel-opts="console=ttyS0 noapic reboot=k panic=1 pci=off nomodules rw"
    '';
}
