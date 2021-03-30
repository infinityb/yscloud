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
    fileSystems = {};
  };

  virtualisation.macaddr = "2e:d3:04:52:b1:af";

  networking.enableIPv6 = true;
  networking.useDHCP = true;
  networking.dhcpcd.persistent = true;
  networking.firewall.enable = false;

  services.postgresql = {
    enable = true;
    package = pkgs.postgresql_13;
  };

  services.hydra = {
    enable = true;
    hydraURL = "http://localhost:3000";
    notificationSender = "hydra@localhost";
    buildMachinesFiles = [];
    useSubstitutes = true;
  };

  # nix.distributedBuilds = true;
  # nix.buildMachines = [
  #   { hostName = "build-host-0";
  #     maxJobs = 2;
  #     sshKey = "/root/.ssh/id_buildfarm";
  #     sshUser = "builder";
  #     system = "x86_64-linux";
  #   }
  # ];

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

  systemd.services.hydra-manual-setup = {
    description = "First launch setup for Hydra";
    serviceConfig.Type = "oneshot";
    serviceConfig.RemainAfterExit = true;
    wantedBy = [ "multi-user.target" ];
    requires = [ "hydra-init.service" ];
    after = [ "hydra-init.service" ];
    # environment = pkgs.lib.mkForce config.systemd.services.hydra-init.environment;
    script = ''
      if [ ! -e ~hydra/.setup-is-complete ]; then
        /run/current-system/sw/bin/hydra-create-user admin --full-name 'Hydra Admin' --email-address 'builds@yshi.org' --password foobar --role admin
        touch ~hydra/.setup-is-complete
      fi
    '';
  };
}