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
in rec {
  platformImage = (import ./platform-image.nix {
    configuration = ./sample.nix;
    extraPackages = [
      rustSource.allWorkspaceMembers
      pkgs.cryptsetup
      pkgs.zfs
    ];
  });

  persistFilesystemRaw = pkgs.runCommand "persist-filesystem" {
    envVariable = true;
  } ''
    set -e
    ${pkgs.coreutils}/bin/mkdir rootfs
    # use cow if we can (`--reflink=auto`) otherwise fall back to copy
    ${pkgs.coreutils}/bin/cp --reflink=auto ${platformImage.image} rootfs/nix-store.squashfs
    ${pkgs.e2fsprogs}/bin/mke2fs -L persist -d rootfs $out 80G
  '';

  persistFilesystemQemu = pkgs.runCommand "persist-filesystem-qcow2" {
    envVariable = true;
  } ''
    ${pkgs.qemu}/bin/qemu-img convert -f raw -O qcow2 ${persistFilesystemRaw} $out
  '';

  # # HACK: add ${pkgs.linuxPackages.zfs} here if we need the zfs modules
  # # 
  # bootStageHelper = (platformImage.withPkgs pkgs).bootStageHelper;
  # initrd = (platformImage.withPkgs pkgs).initrd;
  # startScriptQemu = (platformImage.withPkgs pkgs).startScriptQemu;
  # # startScriptCloudHypervisor = (platformImage.withPkgs pkgs).startScriptCloudHypervisor;
  # startScriptCloudHypervisorTest = (platformImage.withPkgs pkgs).startScriptCloudHypervisorTest;
  # printMacAddr = (platformImage.withPkgs pkgs).printMacAddr;
  # platformImageQcow2 = (platformImage.withPkgs pkgs).platformImageQcow2;

  modulesClosure = pkgs.makeModulesClosure {
    rootModules = [
      # Standard hardware
      "igb" "ixgbe" "e1000"

      # disk
      "dm_verity" "dm_mod" "loop"

      # Virtualization compatibility
      "virtio-net" "virtio-pci" "virtio-blk" "virtio-scsi" "virtio_rng"

      "squashfs"
      "overlay"
      "ext4"
    ];
    kernel = pkgs.linuxPackages.kernel;
    firmware = [];
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
      LINK /usr/sbin/losetup ${pkgs.busybox}/bin/losetup
      LINK /usr/sbin/modprobe ${pkgs.kmod}/bin/modprobe
      LINK /usr/bin/find ${pkgs.busybox}/bin/find
      LINK /usr/sbin/netman ${rustSource.workspaceMembers."appliance-netman".build}/bin/appliance-netman
      MKDIR /bin
      LINK /bin/bash ${pkgs.bash}/bin/bash
    '';

  initrd = pkgs.makeInitrd {
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

  platformImageQcow2 = pkgs.runCommand "platform-image-qcow2" {
    envVariable = true;
  } ''
    ${pkgs.qemu}/bin/qemu-img convert -f raw -O qcow2 ${platformImage.image} $out
  '';

  startScriptQemu = pkgs.writeScriptBin "run-vm"
    ''
    ${pkgs.qemu}/bin/qemu-system-x86_64 \
      -m 4000 \
      -nographic -serial mon:stdio \
      -append 'console=ttyS0 root=/dev/disk/by-path/virtio-pci-0000:00:04.0 nixos-system=${platformImage.toplevel}' \
      -netdev user,id=n1 -device virtio-net-pci,netdev=n1 \
      -kernel ${pkgs.linuxPackages.kernel}/bzImage \
      -drive format=qcow2,if=virtio,file=${platformImageQcow2},readonly=on \
      -initrd ${initrd}/initrd.gz
    '';

  startScriptFirecracker = pkgs.writeScriptBin "run-vm"
    ''
    ${pkgs.firectl}/bin/firectl -c4 -m1024 \
      --firecracker-binary=${pkgs.firecracker}/bin/firecracker \
      --kernel=${pkgs.linuxPackages.kernel}/bzImage \
      --root-drive=${initrd}/initrd.gz \
      --cpu-template=T2 \
      --firecracker-log=firecracker-vmm.log \
      --kernel-opts="console=ttyS0 nixos-system=${platformImage.toplevel} noapic reboot=k panic=1 pci=off rw"
    '';

  printMacAddr = pkgs.writeScriptBin "print-mac"
    ''
    echo ${platformImage.eval.config.virtualisation.macaddr}
    '';

  startScriptCloudHypervisorTest = pkgs.writeScriptBin "run-vm"
    ''
    ${pkgs.cloud-hypervisor}/bin/cloud-hypervisor \
      --serial tty --console off \
      --cpus boot=4 \
      --memory size=1024M \
      --kernel=${pkgs.linuxPackages.kernel}/bzImage \
      --initramfs=${initrd}/initrd.gz \
      --disk path=${image},readonly=on,id=0 \
      --net "tap=,mac=${platformImage.eval.config.virtualisation.macaddr},ip=,mask=" --rng \
      --cmdline="console=ttyS0 root=/dev/vda nixos-system=${platformImage.toplevel}"
    '';

  startScriptCloudHypervisor = pkgs.writeScriptBin "run-vm"
    ''
    (test ! -e ./persist.qcow2 && cp --reflink=auto ${persistFilesystemQemu} ./persist.qcow2; true)
    ${pkgs.cloud-hypervisor}/bin/cloud-hypervisor \
      --serial tty --console off \
      --cpus boot=12 \
      --memory size=12000M \
      --kernel=${pkgs.linuxPackages.kernel}/bzImage \
      --initramfs=${initrd}/initrd.gz \
      --disk path=./persist.qcow2,readonly=off \
      --net "tap=,mac=${platformImage.eval.config.virtualisation.macaddr},ip=,mask=" --rng \
      --cmdline="console=ttyS0 yscloud.personality=persist root=/dev/vda nixos-system=${platformImage.toplevel}"
    '';
}
