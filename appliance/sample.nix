{
  pkgs,
  ...
}:
{
  imports = [
    ./qemu-vm.nix
  ];

  services.openssh.enable = true;

  virtualisation = {
    writableStore = true;
    fileSystems = {
      # "/nix/.rw-store" = {
      #   fsType = "tmpfs";
      #   options = [ "mode=0755" ];
      #   neededForBoot = true;
      # };
      # "/nix/.ro-store" = {
      #   device = "/dev/disk/by-path/virtio-pci-0000:00:04.0";
      #   fsType = "squashfs";
      #   options = [ "loop" ];
      #   neededForBoot = true;
      # };
    };
  };

  # "shark123123"
  users.users."root".initialHashedPassword = "$6$3eNw0.fMLD0e281n$9g4geVRlsxipj09D2x1LED2yq6mg02jCsS2kZDzK6.rhrtIfoO2eb6oK27a9TUUNKxgiYEN4zTL51pTsZt8f8.";

  users.extraUsers.sell = {
    isNormalUser = true;
    home = "/home/sell";
    extraGroups = [ "wheel" ];
    openssh.authorizedKeys.keys = [
      "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILI+WddCXnpxDXQsP1FZpimg+Y8080lVPWhz9xfbEsMQ sell"
    ];
    initialHashedPassword = "$6$3eNw0.fMLD0e281n$9g4geVRlsxipj09D2x1LED2yq6mg02jCsS2kZDzK6.rhrtIfoO2eb6oK27a9TUUNKxgiYEN4zTL51pTsZt8f8.";
  };

  # environment.kernelModules = [ "loop" "overlay" "zfs" ];
}