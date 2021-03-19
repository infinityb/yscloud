with (import <nixpkgs> {});
let
  rustSource = import ../default.nix;
  makeSquashImage = import ../nixlib/make-squashfs.nix;
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
      "virtio-net" "virtio-pci" "virtio-blk" "virtio-scsi"

      "squashfs"
      "overlay"
      "ext4"
    ];
    kernel = pkgs.linuxPackages.kernel;
    firmware = [];
  };

in rec {
  # platformImage = makeSquashImage {
  #   configuration = {
  #     packages = [
  #       rustSource.allWorkspaceMembers
  #       pkgs.linuxPackages.kernel

  #       pkgs.zfs
  #       pkgs.linuxPackages.zfs
  #     ];
  #   };
  # };

  platformImage = import ./platform-image.nix {
    configuration = ./sample.nix;
    extraPackages = [
      rustSource.allWorkspaceMembers
      pkgs.zfs
    ];
  };

  platformImageQcow2 = pkgs.runCommand "platform-image-qcow2" {
    envVariable = true;
  } ''
    ${pkgs.qemu}/bin/qemu-img convert -f raw -O qcow2 ${platformImage} $out
  '';

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
      MKDIR /bin
      LINK /bin/bash ${pkgs.bash}/bin/bash
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

  startScriptQemu = pkgs.writeScriptBin "run-vm"
    ''
    qemu-system-x86_64 \
      -m 4000 \
      -nographic -serial mon:stdio \
      -append 'console=ttyS0 nixos-system=${platformImage.toplevel}' \
      -netdev user,id=n1 -device virtio-net-pci,netdev=n1 \
      -kernel ${pkgs.linuxPackages.kernel}/bzImage \
      -drive format=qcow2,if=virtio,file=${platformImageQcow2},readonly=on \
      -initrd ${initrd}/initrd.gz
    '';

  # startScriptFirecracker = pkgs.writeScriptBin "run-vm"
  #   ''
  #   ${pkgs.firectl}/bin/firectl -c4 -m1024 \
  #     --firecracker-binary=${pkgs.firecracker}/bin/firecracker \
  #     --kernel=${pkgs.linuxPackages.kernel}/bzImage \
  #     --root-drive=${initrd}/initrd.gz \
  #     --cpu-template=T2 \
  #     --firecracker-log=firecracker-vmm.log \
  #     --kernel-opts="console=ttyS0 noapic reboot=k panic=1 pci=off rw"
  #   '';
}
