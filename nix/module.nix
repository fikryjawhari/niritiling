{ config, lib, ... }:

let
  cfg = config.services.niritiling;
in
{
  options.services.niritiling = {
    enable = lib.mkEnableOption "automatic window tiling for the first window in Niri";

    resizeColumns = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "When a column is resized, adjust the other column to compensate (only works with exactly two columns).";
    };

    systemdTarget = lib.mkOption {
      type = lib.types.str;
      default = "graphical-session.target";
      description = "The systemd target to bind the niritiling service to.";
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.user.services.niritiling = {
      description = "niritiling - first-window tiling service for Niri";
      partOf = [ cfg.systemdTarget ];
      after = [ cfg.systemdTarget ];
      wantedBy = [ cfg.systemdTarget ];

      serviceConfig = {
        ExecStart = "${cfg.package}/bin/niritiling${lib.optionalString cfg.resizeColumns " --resize-columns"}";
        Restart = "on-failure";
        RestartSec = 2;

        CapabilityBoundingSet = "";
        IPAddressDeny = "any";
        KeyringMode = "private";
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        NoNewPrivileges = true;
        PrivateDevices = true;
        PrivateNetwork = true;
        PrivateTmp = true;
        PrivateUsers = true;
        ProcSubset = "pid";
        ProtectClock = true;
        ProtectControlGroups = true;
        ProtectHome = "read-only";
        ProtectHostname = true;
        ProtectKernelLogs = true;
        ProtectKernelModules = true;
        ProtectKernelTunables = true;
        ProtectNetwork = true;
        ProtectProc = "invisible";
        ProtectSystem = "strict";
        RestrictAddressFamilies = [ "AF_UNIX" ];
        RestrictNamespaces = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        SystemCallArchitectures = "native";
        SystemCallFilter = "@system-service";
        UMask = "0077";
      };
    };
  };
}
